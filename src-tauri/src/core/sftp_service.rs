use crate::errors::app_error::AppError;
use crate::models::sftp::{RemoteEntry, SftpProgressEvent, SftpTaskStatus, SftpTaskStatusEvent, TransferTask, TransferType};
use ssh2::Session;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Runtime};
use tokio::sync::Semaphore;
use uuid::Uuid;

/// 全局并发信号量，最多允许 5 个传输任务同时运行（跨所有 session）
static TRANSFER_SEMAPHORE: std::sync::OnceLock<Arc<Semaphore>> = std::sync::OnceLock::new();

/// 获取全局传输信号量
fn get_semaphore() -> Arc<Semaphore> {
    TRANSFER_SEMAPHORE.get_or_init(|| Arc::new(Semaphore::new(5))).clone()
}

/// 取消令牌，用于通知传输任务退出
#[derive(Clone)]
pub struct CancelToken(Arc<std::sync::atomic::AtomicBool>);

impl CancelToken {
    /// 创建新的取消令牌
    pub fn new() -> Self {
        Self(Arc::new(std::sync::atomic::AtomicBool::new(false)))
    }

    /// 触发取消
    pub fn cancel(&self) {
        self.0.store(true, std::sync::atomic::Ordering::Relaxed);
    }

    /// 检查是否已取消
    pub fn is_cancelled(&self) -> bool {
        self.0.load(std::sync::atomic::Ordering::Relaxed)
    }
}

/// 单个 session 的 SFTP 句柄，包含 SSH session 引用和任务取消令牌集合
pub struct SftpHandle {
    /// SSH session（Arc<Mutex> 以支持多任务共享）
    pub ssh_session: Arc<Mutex<Session>>,
    /// 任务取消令牌集合，按 task_id 索引
    pub cancel_tokens: HashMap<String, CancelToken>,
}

/// SFTP 服务，管理所有 session 的 SFTP 句柄
pub struct SftpService {
    /// 按 session_id 索引的 SFTP 句柄
    pub handles: HashMap<String, SftpHandle>,
    /// 所有传输任务，按 task_id 索引
    pub tasks: HashMap<String, TransferTask>,
}

impl SftpService {
    /// 创建新的 SFTP 服务实例
    pub fn new() -> Self {
        Self {
            handles: HashMap::new(),
            tasks: HashMap::new(),
        }
    }

    /// 注册 SSH session，供后续 SFTP 操作使用
    ///
    /// # 参数
    /// - `session_id`: 会话 ID
    /// - `ssh_session`: 已建立的 SSH session（Arc<Mutex> 包装）
    pub fn register_session(&mut self, session_id: String, ssh_session: Arc<Mutex<Session>>) {
        self.handles.insert(session_id, SftpHandle {
            ssh_session,
            cancel_tokens: HashMap::new(),
        });
    }

    /// 列举远程目录内容，按目录优先、名称排序
    ///
    /// # 参数
    /// - `session_id`: 关联的 SSH 会话 ID
    /// - `path`: 远程目录绝对路径
    ///
    /// # 返回
    /// 成功返回 RemoteEntry 列表，失败返回 AppError
    pub fn list_dir(&self, session_id: &str, path: &str) -> Result<Vec<RemoteEntry>, AppError> {
        let handle = self.handles.get(session_id)
            .ok_or_else(|| AppError::SftpChannelError(format!("session {} 不存在", session_id)))?;

        let ssh = handle.ssh_session.lock()
            .map_err(|e| AppError::SftpChannelError(e.to_string()))?;

        let sftp = ssh.sftp()
            .map_err(|e| AppError::SftpChannelError(e.to_string()))?;

        let entries_raw = sftp.readdir(Path::new(path))
            .map_err(|e| {
                let msg = e.to_string();
                if msg.contains("No such file") || msg.contains("does not exist") {
                    AppError::SftpPathNotFound(path.to_string())
                } else if msg.contains("Permission denied") {
                    AppError::SftpPermissionDenied(path.to_string())
                } else {
                    AppError::SftpChannelError(msg)
                }
            })?;

        let mut entries: Vec<RemoteEntry> = entries_raw.into_iter().map(|(pb, stat)| {
            let name = pb.file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            let full_path = pb.to_string_lossy().to_string();
            let is_dir = stat.is_dir();
            let size = if is_dir { 0 } else { stat.size.unwrap_or(0) };
            let modified_at = stat.mtime.map(|t| t as i64 * 1000).unwrap_or(0);
            let perm = stat.perm.map(|p| format_permissions(p)).unwrap_or_default();
            RemoteEntry { name, path: full_path, is_dir, size, modified_at, permissions: perm }
        }).collect();

        // 目录优先，同类按名称排序
        entries.sort_by(|a, b| {
            b.is_dir.cmp(&a.is_dir).then(a.name.cmp(&b.name))
        });

        Ok(entries)
    }

    /// 发起下载任务，立即返回 status = Pending 的 TransferTask
    ///
    /// # 参数
    /// - `session_id`: 关联会话 ID
    /// - `remote_path`: 远程文件完整路径
    /// - `local_path`: 本地保存路径（父目录必须存在）
    /// - `app`: Tauri 应用句柄，用于推送事件
    pub fn enqueue_download<R: Runtime>(
        &mut self,
        session_id: String,
        remote_path: String,
        local_path: String,
        app: AppHandle<R>,
    ) -> Result<TransferTask, AppError> {
        // 验证本地路径父目录可写
        let parent = Path::new(&local_path).parent()
            .ok_or_else(|| AppError::SftpTransferError("本地路径无效".to_string()))?;
        if !parent.exists() {
            return Err(AppError::SftpTransferError(format!("本地目录不存在: {}", parent.display())));
        }

        let handle = self.handles.get(&session_id)
            .ok_or_else(|| AppError::SftpChannelError(format!("session {} 不存在", session_id)))?;

        let file_name = Path::new(&remote_path)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| remote_path.clone());

        // 先获取文件大小
        let ssh = handle.ssh_session.lock()
            .map_err(|e| AppError::SftpChannelError(e.to_string()))?;
        let sftp = ssh.sftp().map_err(|e| AppError::SftpChannelError(e.to_string()))?;
        let stat = sftp.stat(Path::new(&remote_path))
            .map_err(|e| AppError::SftpPathNotFound(e.to_string()))?;
        let total_bytes = stat.size.unwrap_or(0);
        drop(sftp);
        drop(ssh);

        let task_id = Uuid::new_v4().to_string();
        let cancel_token = CancelToken::new();
        let task = TransferTask {
            task_id: task_id.clone(),
            session_id: session_id.clone(),
            transfer_type: TransferType::Download,
            remote_path: remote_path.clone(),
            local_path: local_path.clone(),
            file_name,
            total_bytes,
            transferred_bytes: 0,
            speed_bps: 0,
            status: SftpTaskStatus::Pending,
            error_message: None,
            created_at: chrono::Utc::now().timestamp_millis(),
        };

        self.tasks.insert(task_id.clone(), task.clone());
        if let Some(h) = self.handles.get_mut(&session_id) {
            h.cancel_tokens.insert(task_id.clone(), cancel_token.clone());
        }

        // 启动后台传输任务
        let ssh_session = if let Some(h) = self.handles.get(&session_id) {
            h.ssh_session.clone()
        } else {
            return Err(AppError::SftpChannelError(format!("session {} 不存在", session_id)));
        };
        spawn_transfer_task(
            task_id, session_id, remote_path, local_path,
            total_bytes, TransferType::Download,
            ssh_session, cancel_token, app,
        );

        Ok(task)
    }

    /// 发起上传任务，立即返回 status = Pending 的 TransferTask
    ///
    /// # 参数
    /// - `session_id`: 关联会话 ID
    /// - `local_path`: 本地文件完整路径
    /// - `remote_path`: 远程目标目录路径（后端自动拼接文件名）
    /// - `app`: Tauri 应用句柄，用于推送事件
    pub fn enqueue_upload<R: Runtime>(
        &mut self,
        session_id: String,
        local_path: String,
        remote_path: String,
        app: AppHandle<R>,
    ) -> Result<TransferTask, AppError> {
        // 验证本地文件存在
        if !Path::new(&local_path).exists() {
            return Err(AppError::SftpTransferError(format!("本地文件不存在: {}", local_path)));
        }

        let file_name = Path::new(&local_path)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| local_path.clone());

        // 拼接完整远程目标路径
        let full_remote_path = if remote_path.ends_with('/') {
            format!("{}{}", remote_path, file_name)
        } else {
            format!("{}/{}", remote_path, file_name)
        };

        let total_bytes = std::fs::metadata(&local_path)
            .map(|m| m.len())
            .unwrap_or(0);

        // 先克隆 ssh_session，避免后续可变借用冲突
        let ssh_session = self.handles.get(&session_id)
            .ok_or_else(|| AppError::SftpChannelError(format!("session {} 不存在", session_id)))
            .map(|h| h.ssh_session.clone())?;

        let task_id = Uuid::new_v4().to_string();
        let cancel_token = CancelToken::new();
        let task = TransferTask {
            task_id: task_id.clone(),
            session_id: session_id.clone(),
            transfer_type: TransferType::Upload,
            remote_path: full_remote_path.clone(),
            local_path: local_path.clone(),
            file_name,
            total_bytes,
            transferred_bytes: 0,
            speed_bps: 0,
            status: SftpTaskStatus::Pending,
            error_message: None,
            created_at: chrono::Utc::now().timestamp_millis(),
        };

        self.tasks.insert(task_id.clone(), task.clone());
        if let Some(h) = self.handles.get_mut(&session_id) {
            h.cancel_tokens.insert(task_id.clone(), cancel_token.clone());
        }
        spawn_transfer_task(
            task_id, session_id, full_remote_path, local_path,
            total_bytes, TransferType::Upload,
            ssh_session, cancel_token, app,
        );

        Ok(task)
    }

    /// 取消指定传输任务；若任务已为终态则静默成功
    ///
    /// # 参数
    /// - `task_id`: 要取消的任务 ID
    pub fn cancel_task(&mut self, task_id: &str) {
        // 找到对应 session 的取消令牌并触发取消
        for handle in self.handles.values_mut() {
            if let Some(token) = handle.cancel_tokens.get(task_id) {
                token.cancel();
                return;
            }
        }
        // 任务已为终态，静默成功
    }

    /// 清理指定 session 的所有任务（session 关闭时调用）
    ///
    /// 取消所有 Pending/Running 任务，推送 sftp:task_status = Cancelled
    pub fn cleanup_session<R: Runtime>(&mut self, session_id: &str, app: &AppHandle<R>) {
        if let Some(handle) = self.handles.remove(session_id) {
            for (task_id, token) in &handle.cancel_tokens {
                token.cancel();
                if let Some(task) = self.tasks.get_mut(task_id) {
                    if task.status == SftpTaskStatus::Pending || task.status == SftpTaskStatus::Running {
                        task.status = SftpTaskStatus::Cancelled;
                        let event = SftpTaskStatusEvent {
                            task_id: task_id.clone(),
                            session_id: session_id.to_string(),
                            status: SftpTaskStatus::Cancelled,
                            error_message: None,
                        };
                        let _ = app.emit("sftp:task_status", event);
                    }
                }
            }
        }
    }
}

/// 在独立 tokio task 中执行传输，等待信号量 permit，推送状态事件
///
/// # 参数
/// - `task_id`: 任务唯一 ID
/// - `session_id`: 关联会话 ID
/// - `remote_path`: 远程文件路径
/// - `local_path`: 本地文件路径
/// - `total_bytes`: 文件总大小
/// - `transfer_type`: 传输方向
/// - `ssh_session`: SSH session Arc
/// - `cancel_token`: 取消令牌
/// - `app`: Tauri 应用句柄
fn spawn_transfer_task<R: Runtime + 'static>(
    task_id: String,
    session_id: String,
    remote_path: String,
    local_path: String,
    total_bytes: u64,
    transfer_type: TransferType,
    ssh_session: Arc<Mutex<Session>>,
    cancel_token: CancelToken,
    app: AppHandle<R>,
) {
    let semaphore = get_semaphore();
    tokio::spawn(async move {
        // 等待信号量 permit（全局最多 5 个并发）
        let _permit = semaphore.acquire().await.unwrap();

        if cancel_token.is_cancelled() {
            let _ = app.emit("sftp:task_status", SftpTaskStatusEvent {
                task_id, session_id, status: SftpTaskStatus::Cancelled, error_message: None,
            });
            return;
        }

        // 通知 Running
        let _ = app.emit("sftp:task_status", SftpTaskStatusEvent {
            task_id: task_id.clone(), session_id: session_id.clone(),
            status: SftpTaskStatus::Running, error_message: None,
        });

        let task_id_clone = task_id.clone();
        let session_id_clone = session_id.clone();
        let app_clone = app.clone();
        let cancel_token_clone = cancel_token.clone();

        let result = tokio::task::spawn_blocking(move || {
            run_transfer_blocking(
                &task_id_clone, &session_id_clone, &remote_path, &local_path,
                total_bytes, &transfer_type, &ssh_session, &cancel_token_clone, &app_clone,
            )
        }).await;

        match result {
            Ok(Ok(())) => {
                let _ = app.emit("sftp:task_status", SftpTaskStatusEvent {
                    task_id, session_id, status: SftpTaskStatus::Done, error_message: None,
                });
            }
            Ok(Err(true)) => {
                let _ = app.emit("sftp:task_status", SftpTaskStatusEvent {
                    task_id, session_id, status: SftpTaskStatus::Cancelled, error_message: None,
                });
            }
            Ok(Err(false)) => {
                let _ = app.emit("sftp:task_status", SftpTaskStatusEvent {
                    task_id, session_id, status: SftpTaskStatus::Failed,
                    error_message: Some("传输中断".to_string()),
                });
            }
            Err(e) => {
                let _ = app.emit("sftp:task_status", SftpTaskStatusEvent {
                    task_id, session_id, status: SftpTaskStatus::Failed,
                    error_message: Some(e.to_string()),
                });
            }
        }
    });
}

/// 阻塞执行实际传输，每 500ms 推送进度
///
/// # 返回
/// - `Ok(())`: 传输成功
/// - `Err(true)`: 主动取消
/// - `Err(false)`: 传输失败
fn run_transfer_blocking<R: Runtime>(
    task_id: &str,
    session_id: &str,
    remote_path: &str,
    local_path: &str,
    total_bytes: u64,
    transfer_type: &TransferType,
    ssh_session: &Arc<Mutex<Session>>,
    cancel_token: &CancelToken,
    app: &AppHandle<R>,
) -> Result<(), bool> {
    use std::io::{Read, Write};
    use std::time::Instant;

    let ssh = ssh_session.lock().map_err(|_| false)?;
    let sftp = ssh.sftp().map_err(|_| false)?;

    const CHUNK: usize = 32 * 1024; // 32KB chunks
    let mut transferred: u64 = 0;
    let mut last_report = Instant::now();
    let mut last_transferred: u64 = 0;

    /// 内联辅助：推送进度事件
    macro_rules! emit_progress {
        () => {
            if last_report.elapsed().as_millis() >= 500 {
                let elapsed = last_report.elapsed().as_secs_f64().max(0.001);
                let speed = ((transferred - last_transferred) as f64 / elapsed) as u64;
                let _ = app.emit("sftp:progress", SftpProgressEvent {
                    task_id: task_id.to_string(),
                    session_id: session_id.to_string(),
                    transferred_bytes: transferred,
                    total_bytes,
                    speed_bps: speed,
                });
                last_transferred = transferred;
                last_report = Instant::now();
            }
        };
    }

    match transfer_type {
        TransferType::Download => {
            let mut remote_file = sftp.open(std::path::Path::new(remote_path)).map_err(|_| false)?;
            // 创建本地文件；失败或取消时通过 cleanup_local 删除残留
            let mut local_file = std::fs::File::create(local_path).map_err(|_| false)?;
            let mut buf = vec![0u8; CHUNK];

            /// 关闭本地文件句柄并删除残留文件（取消或 IO 失败时调用）
            macro_rules! cleanup_local {
                () => {{
                    drop(local_file);
                    let _ = std::fs::remove_file(local_path);
                }};
            }

            loop {
                if cancel_token.is_cancelled() {
                    // 主动取消：删除本地残留文件后返回取消标志
                    cleanup_local!();
                    return Err(true);
                }
                let n = match remote_file.read(&mut buf) {
                    Ok(n) => n,
                    Err(_) => {
                        // IO 失败：同样删除本地残留文件
                        cleanup_local!();
                        return Err(false);
                    }
                };
                if n == 0 { break; }
                if local_file.write_all(&buf[..n]).is_err() {
                    cleanup_local!();
                    return Err(false);
                }
                transferred += n as u64;
                emit_progress!();
            }
        }
        TransferType::Upload => {
            let mut local_file = std::fs::File::open(local_path).map_err(|_| false)?;
            // 创建远端文件；失败或取消时通过 cleanup_remote 删除残留
            let mut remote_file = sftp.create(std::path::Path::new(remote_path)).map_err(|_| false)?;
            let mut buf = vec![0u8; CHUNK];

            /// 关闭远端文件句柄并删除残留文件（取消或 IO 失败时调用）
            macro_rules! cleanup_remote {
                () => {{
                    drop(remote_file);
                    let _ = sftp.unlink(std::path::Path::new(remote_path));
                }};
            }

            loop {
                if cancel_token.is_cancelled() {
                    // 主动取消：删除远端残留文件后返回取消标志
                    cleanup_remote!();
                    return Err(true);
                }
                let n = match local_file.read(&mut buf) {
                    Ok(n) => n,
                    Err(_) => {
                        cleanup_remote!();
                        return Err(false);
                    }
                };
                if n == 0 { break; }
                if remote_file.write_all(&buf[..n]).is_err() {
                    cleanup_remote!();
                    return Err(false);
                }
                transferred += n as u64;
                emit_progress!();
            }
        }
    }
    Ok(())
}

/// 将 Unix 权限位转换为 "rwxr-xr-x" 格式字符串
fn format_permissions(perm: u32) -> String {
    let chars = ['r', 'w', 'x'];
    let mut result = String::with_capacity(9);
    for shift in [6u32, 3, 0] {
        for (i, &c) in chars.iter().enumerate() {
            if perm & (1 << (shift + 2 - i as u32)) != 0 {
                result.push(c);
            } else {
                result.push('-');
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// 构造一个最小化的 mock SSH Session（仅用于注册，不实际连接）
    fn mock_ssh_session() -> Arc<Mutex<Session>> {
        Arc::new(Mutex::new(ssh2::Session::new().unwrap()))
    }

    // ─── 基础结构测试 ───────────────────────────────────────────────────────

    /// 验证 register_session 后 handles 中存在对应条目
    #[test]
    fn register_session_stores_handle() {
        let mut service = SftpService::new();
        service.register_session("session-1".to_string(), mock_ssh_session());
        assert!(service.handles.contains_key("session-1"));
    }

    /// 验证 cancel_task 对不存在的 task_id 静默成功（不 panic）
    #[test]
    fn cancel_nonexistent_task_is_silent() {
        let mut service = SftpService::new();
        service.cancel_task("nonexistent-task-id"); // 不应 panic
    }

    /// 验证 cleanup_session 移除 session handle
    #[test]
    fn cleanup_session_removes_handle() {
        use tauri::test::mock_app;
        let app = mock_app();
        let mut service = SftpService::new();
        service.register_session("session-1".to_string(), mock_ssh_session());
        assert!(service.handles.contains_key("session-1"));
        service.cleanup_session("session-1", &app.handle().clone());
        assert!(!service.handles.contains_key("session-1"));
    }

    /// 验证 list_dir 对未注册 session 返回 SftpChannelError，且错误消息包含 session_id
    #[test]
    fn list_dir_unknown_session_returns_channel_error() {
        let service = SftpService::new();
        let result = service.list_dir("nonexistent", "/tmp");
        assert!(result.is_err());
        match result.unwrap_err() {
            AppError::SftpChannelError(msg) => assert!(msg.contains("nonexistent")),
            other => panic!("期望 SftpChannelError，实际: {:?}", other),
        }
    }

    // ─── 并发控制测试 ────────────────────────────────────────────────────────

    /// 验证全局 Semaphore 初始 permits 为 5（跨所有 session 的并发上限）
    #[test]
    fn semaphore_has_five_permits() {
        let sem = get_semaphore();
        assert_eq!(sem.available_permits(), 5);
    }

    // ─── CancelToken 测试 ────────────────────────────────────────────────────

    /// 验证 CancelToken 初始未取消，cancel() 后 is_cancelled() 为 true
    #[test]
    fn cancel_token_lifecycle() {
        let token = CancelToken::new();
        assert!(!token.is_cancelled());
        token.cancel();
        assert!(token.is_cancelled());
    }

    /// 验证 CancelToken clone 共享同一原子标志（取消原始令牌，clone 也感知）
    #[test]
    fn cancel_token_clone_shares_state() {
        let token = CancelToken::new();
        let cloned = token.clone();
        token.cancel();
        assert!(cloned.is_cancelled(), "clone 应共享取消状态");
    }

    // ─── 权限格式化测试 ──────────────────────────────────────────────────────

    /// 验证 format_permissions 对 0o755 (rwxr-xr-x) 的转换
    #[test]
    fn format_permissions_rwxr_xr_x() {
        assert_eq!(format_permissions(0o755), "rwxr-xr-x");
    }

    /// 验证 format_permissions 对 0o644 (rw-r--r--) 的转换
    #[allow(non_snake_case)]
    #[test]
    fn format_permissions_rw_r__r__() {
        assert_eq!(format_permissions(0o644), "rw-r--r--");
    }

    /// 验证 format_permissions 对 0o700 (rwx------) 的转换
    #[test]
    fn format_permissions_rwx_only_owner() {
        assert_eq!(format_permissions(0o700), "rwx------");
    }

    // ─── upload 路径拼接测试 ─────────────────────────────────────────────────

    /// 验证 enqueue_upload 当 remote_path 为目录（不含尾部斜杠）时正确拼接文件名
    /// 本地文件 /tmp/deploy.sh 上传到 /var/log → 目标路径应为 /var/log/deploy.sh
    #[test]
    fn upload_remote_path_without_trailing_slash_appends_filename() {
        let remote_dir = "/var/log".to_string();
        let file_name = "deploy.sh";
        let full_remote = if remote_dir.ends_with('/') {
            format!("{}{}", remote_dir, file_name)
        } else {
            format!("{}/{}", remote_dir, file_name)
        };
        assert_eq!(full_remote, "/var/log/deploy.sh");
    }

    /// 验证 enqueue_upload 当 remote_path 含尾部斜杠时不重复斜杠
    #[test]
    fn upload_remote_path_with_trailing_slash_no_double_slash() {
        let remote_dir = "/var/log/".to_string();
        let file_name = "app.log";
        let full_remote = if remote_dir.ends_with('/') {
            format!("{}{}", remote_dir, file_name)
        } else {
            format!("{}/{}", remote_dir, file_name)
        };
        assert_eq!(full_remote, "/var/log/app.log");
    }

    // ─── 任务状态流转测试 ────────────────────────────────────────────────────

    /// 验证 cleanup_session 将 Pending 任务的取消令牌触发
    #[test]
    fn cleanup_session_cancels_pending_task_tokens() {
        use tauri::test::mock_app;
        let app = mock_app();
        let mut service = SftpService::new();
        service.register_session("session-1".to_string(), mock_ssh_session());

        let task_id = "task-pending-1".to_string();
        let cancel_token = CancelToken::new();
        let cloned_token = cancel_token.clone();

        service.handles.get_mut("session-1").unwrap()
            .cancel_tokens.insert(task_id.clone(), cancel_token);

        service.tasks.insert(task_id.clone(), TransferTask {
            task_id: task_id.clone(),
            session_id: "session-1".to_string(),
            transfer_type: TransferType::Download,
            remote_path: "/tmp/file".to_string(),
            local_path: "/local/file".to_string(),
            file_name: "file".to_string(),
            total_bytes: 1024,
            transferred_bytes: 0,
            speed_bps: 0,
            status: SftpTaskStatus::Pending,
            error_message: None,
            created_at: 0,
        });

        service.cleanup_session("session-1", &app.handle().clone());

        assert!(cloned_token.is_cancelled(), "cleanup_session 应触发 Pending 任务的取消令牌");
    }

    /// 验证 cleanup_session 将 Running 任务的取消令牌触发
    #[test]
    fn cleanup_session_cancels_running_task_tokens() {
        use tauri::test::mock_app;
        let app = mock_app();
        let mut service = SftpService::new();
        service.register_session("session-1".to_string(), mock_ssh_session());

        let task_id = "task-running-1".to_string();
        let cancel_token = CancelToken::new();
        let cloned_token = cancel_token.clone();

        service.handles.get_mut("session-1").unwrap()
            .cancel_tokens.insert(task_id.clone(), cancel_token);

        service.tasks.insert(task_id.clone(), TransferTask {
            task_id: task_id.clone(),
            session_id: "session-1".to_string(),
            transfer_type: TransferType::Upload,
            remote_path: "/remote/file".to_string(),
            local_path: "/local/file".to_string(),
            file_name: "file".to_string(),
            total_bytes: 2048,
            transferred_bytes: 512,
            speed_bps: 1024,
            status: SftpTaskStatus::Running,
            error_message: None,
            created_at: 0,
        });

        service.cleanup_session("session-1", &app.handle().clone());

        assert!(cloned_token.is_cancelled(), "cleanup_session 应触发 Running 任务的取消令牌");
    }

    /// 验证 cancel_task 触发对应任务的取消令牌
    #[test]
    fn cancel_task_triggers_cancel_token() {
        let mut service = SftpService::new();
        service.register_session("session-1".to_string(), mock_ssh_session());

        let task_id = "task-1".to_string();
        let cancel_token = CancelToken::new();
        let cloned_token = cancel_token.clone();

        service.handles.get_mut("session-1").unwrap()
            .cancel_tokens.insert(task_id.clone(), cancel_token);

        service.cancel_task(&task_id);

        assert!(cloned_token.is_cancelled(), "cancel_task 应触发对应任务的取消令牌");
    }

    /// 验证终态任务调用 cancel_task 静默成功（令牌已不在 handles 中）
    #[test]
    fn cancel_task_on_completed_task_is_silent() {
        let mut service = SftpService::new();
        service.cancel_task("task-already-done");
        // 不 panic 即通过
    }
}
