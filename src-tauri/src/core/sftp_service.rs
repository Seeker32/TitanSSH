use crate::errors::app_error::AppError;
use crate::models::sftp::{RemoteEntry, SftpTaskStatus, SftpTaskStatusEvent, TransferTask};
use ssh2::Session;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Runtime};
use tokio::sync::Semaphore;

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

    /// 构造一个最小化的 mock SSH Session（仅用于注册，不实际连接）
    fn mock_ssh_session() -> Arc<Mutex<Session>> {
        Arc::new(Mutex::new(ssh2::Session::new().unwrap()))
    }

    /// 验证 register_session 后 handles 中存在对应条目
    #[test]
    fn register_session_stores_handle() {
        let mut service = SftpService::new();
        service.register_session("session-1".to_string(), mock_ssh_session());
        assert!(service.handles.contains_key("session-1"));
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

    /// 验证全局 Semaphore 初始 permits 为 5
    #[test]
    fn semaphore_has_five_permits() {
        let sem = get_semaphore();
        assert_eq!(sem.available_permits(), 5);
    }

    /// 验证 CancelToken 初始未取消，cancel() 后 is_cancelled() 为 true
    #[test]
    fn cancel_token_lifecycle() {
        let token = CancelToken::new();
        assert!(!token.is_cancelled());
        token.cancel();
        assert!(token.is_cancelled());
    }

    /// 验证 CancelToken clone 共享同一原子标志
    #[test]
    fn cancel_token_clone_shares_state() {
        let token = CancelToken::new();
        let cloned = token.clone();
        token.cancel();
        assert!(cloned.is_cancelled(), "clone 应共享取消状态");
    }

    /// 验证 format_permissions 对 0o755 (rwxr-xr-x) 的转换
    #[test]
    fn format_permissions_rwxr_xr_x() {
        assert_eq!(format_permissions(0o755), "rwxr-xr-x");
    }

    /// 验证 format_permissions 对 0o644 (rw-r--r--) 的转换
    #[test]
    fn format_permissions_rw_r__r__() {
        assert_eq!(format_permissions(0o644), "rw-r--r--");
    }

    /// 验证 format_permissions 对 0o700 (rwx------) 的转换
    #[test]
    fn format_permissions_rwx_only_owner() {
        assert_eq!(format_permissions(0o700), "rwx------");
    }
}
