use crate::core::ssh_transport::{self, SftpTransport};
use crate::errors::app_error::AppError;
use crate::models::host::{AuthType, HostConfig};
use crate::models::sftp::{
    RemoteEntry, SftpProgressEvent, SftpTaskStatus, SftpTaskStatusEvent, TransferTask, TransferType,
};
use crate::storage::secure_store;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Condvar, Mutex};
use tauri::{AppHandle, Emitter, Runtime};
use tokio::sync::Semaphore;
use uuid::Uuid;

/// 全局并发信号量，最多允许 5 个传输任务同时运行（跨所有 session）
static TRANSFER_SEMAPHORE: std::sync::OnceLock<Arc<Semaphore>> = std::sync::OnceLock::new();

/// 获取全局传输信号量
fn get_semaphore() -> Arc<Semaphore> {
    TRANSFER_SEMAPHORE
        .get_or_init(|| Arc::new(Semaphore::new(5)))
        .clone()
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

type SftpConnector = Arc<dyn Fn(&HostConfig) -> Result<SftpTransport, AppError> + Send + Sync>;

enum ConnectionState {
    Idle,
    Connecting,
    Ready(Arc<Mutex<SftpTransport>>),
    Failed(String),
    Closed,
}

/// 单个 Session 的 SFTP 连接状态；Condvar 让首个请求等待并行 eager 建连。
struct SftpConnection {
    host: HostConfig,
    connector: SftpConnector,
    state: Mutex<ConnectionState>,
    ready: Condvar,
}

impl SftpConnection {
    /// 创建尚未开始连接的状态槽。
    fn new(host: HostConfig, connector: SftpConnector) -> Self {
        Self {
            host,
            connector,
            state: Mutex::new(ConnectionState::Idle),
            ready: Condvar::new(),
        }
    }

    /// 在后台启动首次连接；Session 打开不会被远端 IO 阻塞。
    fn connect_eager(self: &Arc<Self>) {
        let should_start = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if matches!(*state, ConnectionState::Idle) {
                *state = ConnectionState::Connecting;
                true
            } else {
                false
            }
        };
        if !should_start {
            return;
        }

        let connection = self.clone();
        std::thread::spawn(move || {
            let result = (connection.connector)(&connection.host);
            let mut state = connection
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if !matches!(*state, ConnectionState::Closed) {
                *state = match result {
                    Ok(transport) => ConnectionState::Ready(Arc::new(Mutex::new(transport))),
                    Err(error) => ConnectionState::Failed(error.to_string()),
                };
            }
            connection.ready.notify_all();
        });
    }

    /// 获取可用连接；等待 eager 结果，并在已交付失败后的下一次调用同步重连。
    fn get(&self) -> Result<Arc<Mutex<SftpTransport>>, AppError> {
        loop {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            match &*state {
                ConnectionState::Ready(transport) => return Ok(transport.clone()),
                ConnectionState::Connecting => {
                    state = self
                        .ready
                        .wait(state)
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    drop(state);
                }
                ConnectionState::Failed(message) => {
                    let message = message.clone();
                    *state = ConnectionState::Idle;
                    return Err(AppError::SftpChannelError(message));
                }
                ConnectionState::Closed => {
                    return Err(AppError::SftpChannelError("session 已关闭".to_string()));
                }
                ConnectionState::Idle => {
                    *state = ConnectionState::Connecting;
                    drop(state);
                    let result = (self.connector)(&self.host);
                    let mut state = self
                        .state
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    if matches!(*state, ConnectionState::Closed) {
                        self.ready.notify_all();
                        return Err(AppError::SftpChannelError("session 已关闭".to_string()));
                    }
                    match result {
                        Ok(transport) => {
                            let transport = Arc::new(Mutex::new(transport));
                            *state = ConnectionState::Ready(transport.clone());
                            self.ready.notify_all();
                            return Ok(transport);
                        }
                        Err(error) => {
                            *state = ConnectionState::Idle;
                            self.ready.notify_all();
                            return Err(error);
                        }
                    }
                }
            }
        }
    }

    /// 关闭状态槽并丢弃任何迟到连接结果。
    fn close(&self) {
        *self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = ConnectionState::Closed;
        self.ready.notify_all();
    }
}

/// 单个 Session 的 SFTP 句柄，连接与取消令牌均局部串行化。
struct SftpHandle {
    connection: Arc<SftpConnection>,
    cancel_tokens: Mutex<HashMap<String, CancelToken>>,
}

/// File Transfer module，registry 锁不会跨远程 IO seam。
#[derive(Clone)]
pub struct SftpService {
    handles: Arc<Mutex<HashMap<String, Arc<SftpHandle>>>>,
    tasks: Arc<Mutex<HashMap<String, TransferTask>>>,
    connector: SftpConnector,
}

impl SftpService {
    /// 创建使用真实 SSH transport adapter 的 File Transfer module。
    pub fn new() -> Self {
        Self::with_connector(connect_sftp_for_host)
    }

    /// 注入内部连接 adapter，供 transport contract 测试使用。
    pub(crate) fn with_connector(
        connector: impl Fn(&HostConfig) -> Result<SftpTransport, AppError> + Send + Sync + 'static,
    ) -> Self {
        Self {
            handles: Arc::new(Mutex::new(HashMap::new())),
            tasks: Arc::new(Mutex::new(HashMap::new())),
            connector: Arc::new(connector),
        }
    }

    /// 注册 Session 并并行启动独立 SFTP 连接。
    pub fn register_session(&self, session_id: String, host: HostConfig) {
        let connection = Arc::new(SftpConnection::new(host, self.connector.clone()));
        self.handles
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(
                session_id,
                Arc::new(SftpHandle {
                    connection: connection.clone(),
                    cancel_tokens: Mutex::new(HashMap::new()),
                }),
            );
        connection.connect_eager();
    }

    /// 判断指定 Session 是否仍注册在 File Transfer module。
    #[cfg(test)]
    pub(crate) fn has_session(&self, session_id: &str) -> bool {
        self.handles
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .contains_key(session_id)
    }

    /// 读取 Session 句柄副本，随后立即释放 registry 锁。
    fn handle(&self, session_id: &str) -> Result<Arc<SftpHandle>, AppError> {
        self.handles
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(session_id)
            .cloned()
            .ok_or_else(|| AppError::SftpChannelError(format!("session {} 不存在", session_id)))
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
        let transport = self.handle(session_id)?.connection.get()?;
        let mut entries: Vec<RemoteEntry> = transport
            .lock()
            .map_err(|error| AppError::SftpChannelError(error.to_string()))?
            .list_dir(path)?
            .into_iter()
            .map(|entry| {
                let name = Path::new(&entry.path)
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                RemoteEntry {
                    name,
                    path: entry.path,
                    is_dir: entry.is_dir,
                    size: entry.size,
                    modified_at: entry.modified_at,
                    permissions: entry
                        .permissions
                        .map(format_permissions)
                        .unwrap_or_default(),
                }
            })
            .collect();

        // 目录优先，同类按名称排序
        entries.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then(a.name.cmp(&b.name)));

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
        &self,
        session_id: String,
        remote_path: String,
        local_path: String,
        app: AppHandle<R>,
    ) -> Result<TransferTask, AppError> {
        // 验证本地路径父目录可写
        let parent = Path::new(&local_path)
            .parent()
            .ok_or_else(|| AppError::SftpTransferError("本地路径无效".to_string()))?;
        if !parent.exists() {
            return Err(AppError::SftpTransferError(format!(
                "本地目录不存在: {}",
                parent.display()
            )));
        }

        let handle = self.handle(&session_id)?;

        let file_name = Path::new(&remote_path)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| remote_path.clone());

        let transport = handle.connection.get()?;
        let total_bytes = transport
            .lock()
            .map_err(|error| AppError::SftpChannelError(error.to_string()))?
            .file_size(&remote_path)?;

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

        self.tasks
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(task_id.clone(), task.clone());
        handle
            .cancel_tokens
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(task_id.clone(), cancel_token.clone());

        self.spawn_transfer_task(
            task_id,
            session_id,
            remote_path,
            local_path,
            total_bytes,
            TransferType::Download,
            transport,
            cancel_token,
            app,
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
        &self,
        session_id: String,
        local_path: String,
        remote_path: String,
        app: AppHandle<R>,
    ) -> Result<TransferTask, AppError> {
        // 验证本地文件存在
        if !Path::new(&local_path).exists() {
            return Err(AppError::SftpTransferError(format!(
                "本地文件不存在: {}",
                local_path
            )));
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

        let total_bytes = std::fs::metadata(&local_path).map(|m| m.len()).unwrap_or(0);

        let handle = self.handle(&session_id)?;
        let transport = handle.connection.get()?;

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

        self.tasks
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(task_id.clone(), task.clone());
        handle
            .cancel_tokens
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(task_id.clone(), cancel_token.clone());
        self.spawn_transfer_task(
            task_id,
            session_id,
            full_remote_path,
            local_path,
            total_bytes,
            TransferType::Upload,
            transport,
            cancel_token,
            app,
        );

        Ok(task)
    }

    /// 取消指定传输任务；若任务已为终态则静默成功
    ///
    /// # 参数
    /// - `task_id`: 要取消的任务 ID
    pub fn cancel_task(&self, task_id: &str) {
        // 找到对应 session 的取消令牌并触发取消
        let handles: Vec<_> = self
            .handles
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .values()
            .cloned()
            .collect();
        for handle in handles {
            if let Some(token) = handle
                .cancel_tokens
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .get(task_id)
            {
                token.cancel();
                return;
            }
        }
        // 任务已为终态，静默成功
    }

    /// 迁移任务状态：registry 先更新，再发布事件；任务不存在或迁移非法时拒绝。
    ///
    /// 状态机：Pending → {Running, Cancelled}，Running → {Done, Failed, Cancelled}；
    /// Done / Failed / Cancelled 为终态。终态迁移时同步移除取消令牌，
    /// 使 cancel_task 与 cleanup 不再触碰已结束的任务。
    ///
    /// # 参数
    /// - `app`: Tauri 应用句柄，用于推送事件
    /// - `task_id`: 任务 ID
    /// - `session_id`: 关联会话 ID
    /// - `status`: 目标状态
    /// - `error_message`: 失败原因；Failed 时为具体错误描述，其余为 None
    ///
    /// # 返回
    /// true 表示迁移成功且已发布事件；false 表示被拒绝（未知任务或非法迁移）
    fn transition_task<R: Runtime>(
        &self,
        app: &AppHandle<R>,
        task_id: &str,
        session_id: &str,
        status: SftpTaskStatus,
        error_message: Option<String>,
    ) -> bool {
        let mut tasks = self
            .tasks
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(task) = tasks.get_mut(task_id) else {
            return false;
        };
        let legal = matches!(
            (&task.status, &status),
            (SftpTaskStatus::Pending, SftpTaskStatus::Running)
                | (SftpTaskStatus::Pending, SftpTaskStatus::Cancelled)
                | (SftpTaskStatus::Running, SftpTaskStatus::Done)
                | (SftpTaskStatus::Running, SftpTaskStatus::Failed)
                | (SftpTaskStatus::Running, SftpTaskStatus::Cancelled)
        );
        if !legal {
            return false;
        }
        task.status = status.clone();
        task.error_message = error_message.clone();
        drop(tasks);

        // 终态后移除取消令牌；session 已关闭时（cleanup 后）跳过
        if is_terminal(&status) {
            if let Ok(handle) = self.handle(session_id) {
                handle
                    .cancel_tokens
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .remove(task_id);
            }
        }

        let _ = app.emit(
            "sftp:task_status",
            SftpTaskStatusEvent {
                task_id: task_id.to_string(),
                session_id: session_id.to_string(),
                status,
                error_message,
            },
        );
        true
    }

    /// 清理指定 session 的所有任务（session 关闭时调用）
    ///
    /// 只取消并迁移尚未终态的任务（registry 为权威状态），推送一次
    /// sftp:task_status = Cancelled；终态任务不再重复取消或发矛盾事件。
    /// 清理完成后该 session 的任务从 registry 整体移除。
    pub fn cleanup_session<R: Runtime>(&self, session_id: &str, app: &AppHandle<R>) {
        let handle = self
            .handles
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(session_id);
        if let Some(handle) = handle {
            handle.connection.close();
            // 收集该 session 尚未终态的任务（短暂持锁，不跨远程 IO）
            let active: Vec<(String, CancelToken)> = {
                let cancel_tokens = handle
                    .cancel_tokens
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                let tasks = self
                    .tasks
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                cancel_tokens
                    .iter()
                    .filter(|(task_id, _)| {
                        matches!(tasks.get(*task_id), Some(task) if !is_terminal(&task.status))
                    })
                    .map(|(task_id, token)| (task_id.clone(), token.clone()))
                    .collect()
            };
            for (task_id, token) in active {
                token.cancel();
                self.transition_task(app, &task_id, session_id, SftpTaskStatus::Cancelled, None);
            }
            // registry 只保留活任务：session 关闭后整体移除，迟到的 worker 迁移因任务不存在被拒绝
            self.tasks
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .retain(|_, task| task.session_id != session_id);
        }
    }

    /// 在独立 tokio task 中执行传输，等待信号量 permit，通过 transition 更新 registry 并推送状态事件
    ///
    /// # 参数
    /// - `task_id`: 任务唯一 ID
    /// - `session_id`: 关联会话 ID
    /// - `remote_path`: 远程文件路径
    /// - `local_path`: 本地文件路径
    /// - `total_bytes`: 文件总大小
    /// - `transfer_type`: 传输方向
    /// - `transport`: Session 专属 SFTP capability
    /// - `cancel_token`: 取消令牌
    /// - `app`: Tauri 应用句柄
    fn spawn_transfer_task<R: Runtime + 'static>(
        &self,
        task_id: String,
        session_id: String,
        remote_path: String,
        local_path: String,
        total_bytes: u64,
        transfer_type: TransferType,
        transport: Arc<Mutex<SftpTransport>>,
        cancel_token: CancelToken,
        app: AppHandle<R>,
    ) {
        let semaphore = get_semaphore();
        let service = self.clone();
        tokio::spawn(async move {
            // 等待信号量 permit（全局最多 5 个并发）
            let _permit = semaphore.acquire().await.unwrap();

            if cancel_token.is_cancelled() {
                // 入队后即被取消：迁移到 Cancelled；若已由 cleanup 迁移则被拒绝
                service.transition_task(
                    &app,
                    &task_id,
                    &session_id,
                    SftpTaskStatus::Cancelled,
                    None,
                );
                return;
            }

            // 迁移到 Running
            service.transition_task(&app, &task_id, &session_id, SftpTaskStatus::Running, None);

            let task_id_clone = task_id.clone();
            let session_id_clone = session_id.clone();
            let app_clone = app.clone();
            let cancel_token_clone = cancel_token.clone();

            let result = tokio::task::spawn_blocking(move || {
                run_transfer_blocking(
                    &task_id_clone,
                    &session_id_clone,
                    &remote_path,
                    &local_path,
                    total_bytes,
                    &transfer_type,
                    &transport,
                    &cancel_token_clone,
                    &app_clone,
                )
            })
            .await;

            match result {
                Ok(Ok(())) => {
                    service.transition_task(
                        &app,
                        &task_id,
                        &session_id,
                        SftpTaskStatus::Done,
                        None,
                    );
                }
                Ok(Err(true)) => {
                    service.transition_task(
                        &app,
                        &task_id,
                        &session_id,
                        SftpTaskStatus::Cancelled,
                        None,
                    );
                }
                Ok(Err(false)) => {
                    service.transition_task(
                        &app,
                        &task_id,
                        &session_id,
                        SftpTaskStatus::Failed,
                        Some("传输中断".to_string()),
                    );
                }
                Err(e) => {
                    service.transition_task(
                        &app,
                        &task_id,
                        &session_id,
                        SftpTaskStatus::Failed,
                        Some(e.to_string()),
                    );
                }
            }
        });
    }
}

/// 判断任务状态是否为终态（不再接受任何迁移）
fn is_terminal(status: &SftpTaskStatus) -> bool {
    matches!(
        status,
        SftpTaskStatus::Done | SftpTaskStatus::Failed | SftpTaskStatus::Cancelled
    )
}

/// 从 secure storage 读取运行时凭据并建立独立 SFTP transport。
fn connect_sftp_for_host(host: &HostConfig) -> Result<SftpTransport, AppError> {
    let (password, passphrase) = match host.auth_type {
        AuthType::Password => {
            let password_ref = host
                .password_ref
                .as_deref()
                .ok_or_else(|| AppError::InvalidHostConfig("密码引用为空".to_string()))?;
            (Some(secure_store::get_credential(password_ref)?), None)
        }
        AuthType::PrivateKey => {
            let passphrase = host
                .passphrase_ref
                .as_deref()
                .map(secure_store::get_credential)
                .transpose()?;
            (None, passphrase)
        }
    };
    ssh_transport::connect_sftp(host, password.as_deref(), passphrase.as_deref())
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
    transport: &Arc<Mutex<SftpTransport>>,
    cancel_token: &CancelToken,
    app: &AppHandle<R>,
) -> Result<(), bool> {
    use std::io::{Read, Write};
    use std::time::Instant;

    let mut sftp = transport.lock().map_err(|_| false)?;

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
                let _ = app.emit(
                    "sftp:progress",
                    SftpProgressEvent {
                        task_id: task_id.to_string(),
                        session_id: session_id.to_string(),
                        transferred_bytes: transferred,
                        total_bytes,
                        speed_bps: speed,
                    },
                );
                last_transferred = transferred;
                last_report = Instant::now();
            }
        };
    }

    match transfer_type {
        TransferType::Download => {
            let mut remote_file = sftp.open_read(remote_path).map_err(|_| false)?;
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
                if n == 0 {
                    break;
                }
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
            let mut remote_file = sftp.create(remote_path).map_err(|_| false)?;
            let mut buf = vec![0u8; CHUNK];

            /// 关闭远端文件句柄并删除残留文件（取消或 IO 失败时调用）
            macro_rules! cleanup_remote {
                () => {{
                    drop(remote_file);
                    let _ = sftp.unlink(remote_path);
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
                if n == 0 {
                    break;
                }
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
    use crate::core::ssh_transport::test_support::{
        blocking_sftp, drop_signal_sftp, empty_sftp, memory_sftp,
    };
    use crate::models::host::{AuthType, HostConfig};
    use std::sync::atomic::Ordering;
    use std::sync::{Arc, Barrier};
    use std::time::Duration;

    /// 构造不含明文凭据的测试主机。
    fn make_host() -> HostConfig {
        HostConfig {
            id: "host-1".to_string(),
            name: "test".to_string(),
            host: "127.0.0.1".to_string(),
            port: 22,
            username: "root".to_string(),
            auth_type: AuthType::Password,
            password_ref: Some("ref".to_string()),
            private_key_path: None,
            passphrase_ref: None,
            remark: None,
        }
    }

    /// 后台 eager 失败只交付一次，下一次操作必须触发重连。
    #[test]
    fn eager_failure_is_reported_once_then_next_operation_retries() {
        let attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let attempts_for_connector = attempts.clone();
        let service = SftpService::with_connector(move |_| {
            if attempts_for_connector.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                Err(AppError::SshConnectionError("first failure".to_string()))
            } else {
                Ok(empty_sftp())
            }
        });

        service.register_session("session-1".to_string(), make_host());
        let first = service.list_dir("session-1", "/");
        let second = service.list_dir("session-1", "/");

        assert!(first.unwrap_err().to_string().contains("first failure"));
        assert!(second.is_ok());
        assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    /// 一个 Session 的慢目录读取不得持有其他 Session 所需的 registry 锁。
    #[test]
    fn slow_directory_read_does_not_block_another_session() {
        let started = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let connector_started = started.clone();
        let connector_release = release.clone();
        let service = SftpService::with_connector(move |host| {
            if host.id == "slow" {
                Ok(blocking_sftp(
                    connector_started.clone(),
                    connector_release.clone(),
                ))
            } else {
                Ok(empty_sftp())
            }
        });
        let mut slow_host = make_host();
        slow_host.id = "slow".to_string();
        let mut fast_host = make_host();
        fast_host.id = "fast".to_string();
        service.register_session("slow-session".to_string(), slow_host);
        service.register_session("fast-session".to_string(), fast_host);

        let slow_service = service.clone();
        let slow = std::thread::spawn(move || slow_service.list_dir("slow-session", "/"));
        started.wait();

        let fast_service = service.clone();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = done_tx.send(fast_service.list_dir("fast-session", "/"));
        });
        let fast_result = done_rx
            .recv_timeout(Duration::from_millis(200))
            .expect("其他 Session 不应被慢目录读取阻塞");
        release.wait();

        assert!(fast_result.is_ok());
        assert!(slow.join().unwrap().is_ok());
    }

    /// Session 关闭后，阻塞建连的迟到结果必须被释放且不得重新注册。
    #[test]
    fn closed_session_discards_late_sftp_connection() {
        use tauri::test::mock_app;

        let started = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let started_for_connector = started.clone();
        let release_for_connector = release.clone();
        let (dropped_tx, dropped_rx) = std::sync::mpsc::channel();
        let service = SftpService::with_connector(move |_| {
            started_for_connector.wait();
            release_for_connector.wait();
            Ok(drop_signal_sftp(dropped_tx.clone()))
        });
        service.register_session("session-1".to_string(), make_host());
        started.wait();

        let app = mock_app();
        service.cleanup_session("session-1", &app.handle().clone());
        release.wait();

        dropped_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("迟到 transport 应立即释放");
        assert!(!service.has_session("session-1"));
    }

    /// 构造使用内存 SFTP adapter 的测试 module。
    fn make_service() -> SftpService {
        SftpService::with_connector(|_| Ok(empty_sftp()))
    }

    // ─── 基础结构测试 ───────────────────────────────────────────────────────

    /// 验证 register_session 后 handles 中存在对应条目
    #[test]
    fn register_session_stores_handle() {
        let service = make_service();
        service.register_session("session-1".to_string(), make_host());
        assert!(service.has_session("session-1"));
    }

    /// 验证 cancel_task 对不存在的 task_id 静默成功（不 panic）
    #[test]
    fn cancel_nonexistent_task_is_silent() {
        let service = SftpService::new();
        service.cancel_task("nonexistent-task-id"); // 不应 panic
    }

    /// 验证 cleanup_session 移除 session handle
    #[test]
    fn cleanup_session_removes_handle() {
        use tauri::test::mock_app;
        let app = mock_app();
        let service = make_service();
        service.register_session("session-1".to_string(), make_host());
        assert!(service.has_session("session-1"));
        service.cleanup_session("session-1", &app.handle().clone());
        assert!(!service.has_session("session-1"));
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
        let service = make_service();
        service.register_session("session-1".to_string(), make_host());

        let task_id = "task-pending-1".to_string();
        let cancel_token = CancelToken::new();
        let cloned_token = cancel_token.clone();

        service
            .handle("session-1")
            .unwrap()
            .cancel_tokens
            .lock()
            .unwrap()
            .insert(task_id.clone(), cancel_token);

        service.tasks.lock().unwrap().insert(
            task_id.clone(),
            TransferTask {
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
            },
        );

        service.cleanup_session("session-1", &app.handle().clone());

        assert!(
            cloned_token.is_cancelled(),
            "cleanup_session 应触发 Pending 任务的取消令牌"
        );
    }

    /// 验证 cleanup_session 将 Running 任务的取消令牌触发
    #[test]
    fn cleanup_session_cancels_running_task_tokens() {
        use tauri::test::mock_app;
        let app = mock_app();
        let service = make_service();
        service.register_session("session-1".to_string(), make_host());

        let task_id = "task-running-1".to_string();
        let cancel_token = CancelToken::new();
        let cloned_token = cancel_token.clone();

        service
            .handle("session-1")
            .unwrap()
            .cancel_tokens
            .lock()
            .unwrap()
            .insert(task_id.clone(), cancel_token);

        service.tasks.lock().unwrap().insert(
            task_id.clone(),
            TransferTask {
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
            },
        );

        service.cleanup_session("session-1", &app.handle().clone());

        assert!(
            cloned_token.is_cancelled(),
            "cleanup_session 应触发 Running 任务的取消令牌"
        );
    }

    /// 验证 cancel_task 触发对应任务的取消令牌
    #[test]
    fn cancel_task_triggers_cancel_token() {
        let service = make_service();
        service.register_session("session-1".to_string(), make_host());

        let task_id = "task-1".to_string();
        let cancel_token = CancelToken::new();
        let cloned_token = cancel_token.clone();

        service
            .handle("session-1")
            .unwrap()
            .cancel_tokens
            .lock()
            .unwrap()
            .insert(task_id.clone(), cancel_token);

        service.cancel_task(&task_id);

        assert!(
            cloned_token.is_cancelled(),
            "cancel_task 应触发对应任务的取消令牌"
        );
    }

    /// 验证终态任务调用 cancel_task 静默成功（令牌已不在 handles 中）
    #[test]
    fn cancel_task_on_completed_task_is_silent() {
        let service = SftpService::new();
        service.cancel_task("task-already-done");
        // 不 panic 即通过
    }

    // ─── 任务状态迁移权威化测试 ──────────────────────────────────────────────

    /// 构造已注册的传输任务，同时注册取消令牌，返回令牌供断言。
    fn insert_task(service: &SftpService, task_id: &str, status: SftpTaskStatus) -> CancelToken {
        let token = CancelToken::new();
        service
            .handle("session-1")
            .unwrap()
            .cancel_tokens
            .lock()
            .unwrap()
            .insert(task_id.to_string(), token.clone());
        service.tasks.lock().unwrap().insert(
            task_id.to_string(),
            TransferTask {
                task_id: task_id.to_string(),
                session_id: "session-1".to_string(),
                transfer_type: TransferType::Download,
                remote_path: "/tmp/file".to_string(),
                local_path: "/local/file".to_string(),
                file_name: "file".to_string(),
                total_bytes: 1024,
                transferred_bytes: 0,
                speed_bps: 0,
                status,
                error_message: None,
                created_at: 0,
            },
        );
        token
    }

    /// 轮询 registry 直到任务到达终态；超时 panic。
    fn wait_for_terminal(service: &SftpService, task_id: &str) -> SftpTaskStatus {
        for _ in 0..200 {
            let status = service
                .tasks
                .lock()
                .unwrap()
                .get(task_id)
                .map(|task| task.status.clone());
            if let Some(status) = status {
                if matches!(
                    status,
                    SftpTaskStatus::Done | SftpTaskStatus::Failed | SftpTaskStatus::Cancelled
                ) {
                    return status;
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("任务 {} 未在 2 秒内到达终态", task_id);
    }

    /// 迁移必须先更新 registry 再发布事件；终态后取消令牌被移除。
    #[test]
    fn transition_updates_registry_emits_and_removes_token_on_terminal() {
        use tauri::test::mock_app;

        let app = mock_app();
        let service = make_service();
        service.register_session("session-1".to_string(), make_host());
        let token = insert_task(&service, "task-1", SftpTaskStatus::Pending);

        assert!(service.transition_task(
            &app.handle(),
            "task-1",
            "session-1",
            SftpTaskStatus::Running,
            None
        ));
        assert_eq!(
            service.tasks.lock().unwrap().get("task-1").unwrap().status,
            SftpTaskStatus::Running
        );

        assert!(service.transition_task(
            &app.handle(),
            "task-1",
            "session-1",
            SftpTaskStatus::Done,
            None
        ));
        let task = service.tasks.lock().unwrap().get("task-1").unwrap().clone();
        assert_eq!(task.status, SftpTaskStatus::Done);
        assert!(task.error_message.is_none());
        assert!(
            service
                .handle("session-1")
                .unwrap()
                .cancel_tokens
                .lock()
                .unwrap()
                .get("task-1")
                .is_none(),
            "终态后取消令牌应从 handles 移除"
        );
        assert!(!token.is_cancelled());
    }

    /// 终态任务拒绝继续迁移，且不得再发事件。
    #[test]
    fn terminal_task_rejects_further_transitions() {
        use std::sync::atomic::AtomicUsize;
        use tauri::Listener;
        use tauri::test::mock_app;

        let app = mock_app();
        let service = make_service();
        service.register_session("session-1".to_string(), make_host());
        insert_task(&service, "task-1", SftpTaskStatus::Done);
        let emitted = Arc::new(AtomicUsize::new(0));
        let emitted_ref = emitted.clone();
        app.listen("sftp:task_status", move |_| {
            emitted_ref.fetch_add(1, Ordering::Relaxed);
        });

        assert!(!service.transition_task(
            &app.handle(),
            "task-1",
            "session-1",
            SftpTaskStatus::Running,
            None
        ));
        assert_eq!(
            service.tasks.lock().unwrap().get("task-1").unwrap().status,
            SftpTaskStatus::Done
        );
        assert_eq!(emitted.load(Ordering::Relaxed), 0, "终态后不得再发事件");
    }

    /// registry 中不存在的任务迁移被拒绝且不发事件。
    #[test]
    fn transition_rejected_for_unknown_task_emits_nothing() {
        use std::sync::atomic::AtomicUsize;
        use tauri::Listener;
        use tauri::test::mock_app;

        let app = mock_app();
        let service = make_service();
        service.register_session("session-1".to_string(), make_host());
        let emitted = Arc::new(AtomicUsize::new(0));
        let emitted_ref = emitted.clone();
        app.listen("sftp:task_status", move |_| {
            emitted_ref.fetch_add(1, Ordering::Relaxed);
        });

        assert!(!service.transition_task(
            &app.handle(),
            "ghost-task",
            "session-1",
            SftpTaskStatus::Done,
            None
        ));
        assert_eq!(emitted.load(Ordering::Relaxed), 0);
    }

    /// cleanup_session 不得重复取消已终态任务，也不得发矛盾事件；任务随后从 registry 移除。
    #[test]
    fn cleanup_session_skips_terminal_tasks() {
        use std::sync::atomic::AtomicUsize;
        use tauri::Listener;
        use tauri::test::mock_app;

        let app = mock_app();
        let service = make_service();
        service.register_session("session-1".to_string(), make_host());
        let token = insert_task(&service, "task-1", SftpTaskStatus::Done);
        let emitted = Arc::new(AtomicUsize::new(0));
        let emitted_ref = emitted.clone();
        app.listen("sftp:task_status", move |_| {
            emitted_ref.fetch_add(1, Ordering::Relaxed);
        });

        service.cleanup_session("session-1", &app.handle().clone());

        assert!(
            !token.is_cancelled(),
            "终态任务的取消令牌不应被 cleanup 触发"
        );
        assert_eq!(
            emitted.load(Ordering::Relaxed),
            0,
            "cleanup 不应为终态任务发 Cancelled 事件"
        );
        assert!(
            !service.tasks.lock().unwrap().contains_key("task-1"),
            "cleanup 应从 registry 移除该 session 的任务"
        );
    }

    /// cleanup_session 对非终态任务：触发令牌、迁移到 Cancelled 并发事件。
    #[test]
    fn cleanup_session_cancels_active_task_with_event() {
        use std::sync::atomic::AtomicUsize;
        use tauri::Listener;
        use tauri::test::mock_app;

        let app = mock_app();
        let service = make_service();
        service.register_session("session-1".to_string(), make_host());
        let token = insert_task(&service, "task-1", SftpTaskStatus::Running);
        let emitted = Arc::new(AtomicUsize::new(0));
        let emitted_ref = emitted.clone();
        app.listen("sftp:task_status", move |_| {
            emitted_ref.fetch_add(1, Ordering::Relaxed);
        });

        service.cleanup_session("session-1", &app.handle().clone());

        assert!(token.is_cancelled(), "非终态任务的令牌应被 cleanup 触发");
        assert_eq!(emitted.load(Ordering::Relaxed), 1);
        assert!(
            !service.tasks.lock().unwrap().contains_key("task-1"),
            "cleanup 后 registry 不应保留该 session 的任务"
        );
    }

    // ─── worker 更新 registry 的全链路测试 ─────────────────────────────────

    /// 上传 worker 完成真实传输后，registry 由 worker 更新为 Done 并移除令牌。
    #[tokio::test(flavor = "multi_thread")]
    async fn upload_worker_updates_registry_to_done() {
        use tauri::test::mock_app;

        let app = mock_app();
        let service = SftpService::with_connector(|_| Ok(memory_sftp(vec![1u8, 2, 3])));
        service.register_session("session-1".to_string(), make_host());

        let local_path = std::env::temp_dir().join(format!("titan-upload-{}.bin", Uuid::new_v4()));
        std::fs::write(&local_path, b"hello").unwrap();
        let task = service
            .enqueue_upload(
                "session-1".to_string(),
                local_path.to_string_lossy().to_string(),
                "/tmp".to_string(),
                app.handle().clone(),
            )
            .unwrap();

        assert_eq!(
            wait_for_terminal(&service, &task.task_id),
            SftpTaskStatus::Done
        );
        let registry_task = service
            .tasks
            .lock()
            .unwrap()
            .get(&task.task_id)
            .unwrap()
            .clone();
        assert_eq!(registry_task.status, SftpTaskStatus::Done);
        assert!(registry_task.error_message.is_none());
        assert!(
            service
                .handle("session-1")
                .unwrap()
                .cancel_tokens
                .lock()
                .unwrap()
                .get(&task.task_id)
                .is_none(),
            "worker 迁移到终态后应移除取消令牌"
        );
        let _ = std::fs::remove_file(&local_path);
    }

    /// 下载 worker 完成真实传输后，registry 由 worker 更新为 Done，内容写入本地。
    #[tokio::test(flavor = "multi_thread")]
    async fn download_worker_updates_registry_to_done() {
        use tauri::test::mock_app;

        let app = mock_app();
        let service = SftpService::with_connector(|_| Ok(memory_sftp(vec![7u8; 4096])));
        service.register_session("session-1".to_string(), make_host());

        let local_path =
            std::env::temp_dir().join(format!("titan-download-{}.bin", Uuid::new_v4()));
        let task = service
            .enqueue_download(
                "session-1".to_string(),
                "/remote/file.bin".to_string(),
                local_path.to_string_lossy().to_string(),
                app.handle().clone(),
            )
            .unwrap();

        assert_eq!(
            wait_for_terminal(&service, &task.task_id),
            SftpTaskStatus::Done
        );
        assert_eq!(
            std::fs::metadata(&local_path).unwrap().len(),
            4096,
            "下载内容应写入本地文件"
        );
        let _ = std::fs::remove_file(&local_path);
    }

    /// 传输失败时 worker 把 registry 更新为 Failed。
    #[tokio::test(flavor = "multi_thread")]
    async fn failing_worker_updates_registry_to_failed() {
        use tauri::test::mock_app;

        let app = mock_app();
        let service = make_service(); // empty_sftp：open_read/create 失败
        service.register_session("session-1".to_string(), make_host());

        let local_path = std::env::temp_dir().join(format!("titan-fail-{}.bin", Uuid::new_v4()));
        std::fs::write(&local_path, b"x").unwrap();
        let task = service
            .enqueue_upload(
                "session-1".to_string(),
                local_path.to_string_lossy().to_string(),
                "/tmp".to_string(),
                app.handle().clone(),
            )
            .unwrap();

        assert_eq!(
            wait_for_terminal(&service, &task.task_id),
            SftpTaskStatus::Failed
        );
        let _ = std::fs::remove_file(&local_path);
    }
}
