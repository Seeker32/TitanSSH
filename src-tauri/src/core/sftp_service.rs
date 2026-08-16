use crate::core::host_identity::HostKeyVerifier;
use crate::core::ssh_transport::{self, SftpTransport};
use crate::core::transfer_pool::{
    CheckoutError, TRANSFER_IDLE_TIMEOUT, TransferCheckout, TransferClock, TransferPool,
};
use crate::errors::app_error::{AppError, AppErrorInfo, ErrorDetail};
use crate::models::host::{AuthType, HostConfig};
use crate::models::sftp::{
    ConflictStrategy, RemoteEntry, SftpProgressEvent, SftpTaskStatus, SftpTaskStatusEvent,
    TransferTask, TransferType,
};
use crate::storage::secure_store;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Runtime};
use tempfile::{NamedTempFile, TempPath};
use tokio::sync::Semaphore;
use uuid::Uuid;

/// 全局并发传输上限：所有 Session 合计最多 20 个 Running 传输。
/// 每个 Session 内部仍有五路连接上限（MAX_TRANSFER_CONNECTIONS_PER_SESSION），
/// 任务先取得 Session 名额，再竞争全局 permit（tokio 信号量 FIFO 公平）。
const MAX_CONCURRENT_TRANSFERS: usize = 20;

/// 每个 Session 最多保留的终态任务条数（Done/Failed/Cancelled）；Pending/Running 不计入且永不淘汰
const MAX_TERMINAL_TASKS_PER_SESSION: usize = 100;

/// 全局并发信号量，最多允许 MAX_CONCURRENT_TRANSFERS 个传输任务同时运行（跨所有 session）
static TRANSFER_SEMAPHORE: std::sync::OnceLock<Arc<Semaphore>> = std::sync::OnceLock::new();

/// 获取全局传输信号量
fn get_semaphore() -> Arc<Semaphore> {
    TRANSFER_SEMAPHORE
        .get_or_init(|| Arc::new(Semaphore::new(MAX_CONCURRENT_TRANSFERS)))
        .clone()
}

/// 取消令牌，用于通知传输任务退出；异步等待方经 Notify 被立即唤醒，
/// 使等待全局 permit 的任务也能即时终止。
#[derive(Clone)]
pub struct CancelToken {
    cancelled: Arc<std::sync::atomic::AtomicBool>,
    notified: Arc<tokio::sync::Notify>,
}

impl CancelToken {
    /// 创建新的取消令牌
    pub fn new() -> Self {
        Self {
            cancelled: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            notified: Arc::new(tokio::sync::Notify::new()),
        }
    }

    /// 触发取消并唤醒所有等待方；重复调用幂等
    pub fn cancel(&self) {
        self.cancelled
            .store(true, std::sync::atomic::Ordering::Relaxed);
        self.notified.notify_one();
    }

    /// 检查是否已取消
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// 异步等待取消：已取消立即返回，未取消阻塞到 cancel() 触发。
    /// 循环“检查标志 → 等待唤醒”：notify_one 在无等待者注册时暂存 permit，
    /// 取消发生在注册窗口内也不会错过唤醒。
    pub async fn cancelled(&self) {
        loop {
            if self.is_cancelled() {
                return;
            }
            self.notified.notified().await;
        }
    }
}

/// 连接用途：控制连接处理目录/元数据操作，传输连接搬运文件数据。
/// 生产 adapter 对两种用途行为一致；测试 adapter 借此返回不同 capability。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SftpRole {
    /// 控制连接：目录列举、元数据、冲突检查专用
    Control,
    /// 传输连接：上传/下载数据搬运专用
    Transfer,
}

pub(crate) type SftpConnector = Arc<
    dyn Fn(&HostConfig, SftpRole, &HostKeyVerifier) -> Result<SftpTransport, AppError>
        + Send
        + Sync,
>;

enum ConnectionState {
    Idle,
    Connecting,
    Ready(Arc<Mutex<SftpTransport>>),
    Failed(String),
    Closed,
}

/// 单个 Session 的一条 SFTP 连接状态；Condvar 让首个请求等待并行 eager 建连。
/// 同一 Session 的控制连接与传输连接各占一个状态槽，互不持锁。
struct SftpConnection {
    host: HostConfig,
    /// 主机身份统一校验器：握手后、认证前生效，与 Session 生命周期一致
    verifier: HostKeyVerifier,
    connector: SftpConnector,
    role: SftpRole,
    state: Mutex<ConnectionState>,
    ready: Condvar,
}

impl SftpConnection {
    /// 创建尚未开始连接的状态槽。
    fn new(
        host: HostConfig,
        verifier: HostKeyVerifier,
        connector: SftpConnector,
        role: SftpRole,
    ) -> Self {
        Self {
            host,
            verifier,
            connector,
            role,
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
            let result =
                (connection.connector)(&connection.host, connection.role, &connection.verifier);
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
                    return Err(AppError::SftpChannelError(message.into()));
                }
                ConnectionState::Closed => {
                    return Err(AppError::SftpChannelError(ErrorDetail::msg(
                        "session 已关闭",
                        Vec::new(),
                    )));
                }
                ConnectionState::Idle => {
                    *state = ConnectionState::Connecting;
                    drop(state);
                    let result = (self.connector)(&self.host, self.role, &self.verifier);
                    let mut state = self
                        .state
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    if matches!(*state, ConnectionState::Closed) {
                        self.ready.notify_all();
                        return Err(AppError::SftpChannelError(ErrorDetail::msg(
                            "session 已关闭",
                            Vec::new(),
                        )));
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

    /// 仅当当前 Ready 连接正是本次操作使用的 transport 时将其淘汰；
    /// 已被其他操作重建的新连接不受影响，保证同一失效连接只触发一次重建。
    fn invalidate_if_ready(&self, transport: &Arc<Mutex<SftpTransport>>) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let ConnectionState::Ready(current) = &*state
            && Arc::ptr_eq(current, transport)
        {
            *state = ConnectionState::Idle;
        }
        self.ready.notify_all();
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

/// 单个 Session 的 SFTP 句柄；控制与传输 capability 分离，取消令牌局部串行化。
///
/// 控制连接保证目录列举、元数据与冲突检查不被长传输阻塞；
/// 传输连接池从基础一条按需扩展到最多五条，额外连接空闲超时回收。
struct SftpHandle {
    /// 专用控制连接：目录列举、元数据、冲突检查；传输期间绝不被持锁
    control: Arc<SftpConnection>,
    /// 传输连接池：上传/下载专用，基础一条按需建立，最多五条
    transfer_pool: Arc<TransferPool>,
    /// 传输任务取消条目表：cancel_task 同时触发取消并把等待者移出传输池 FIFO 队列
    cancel_tokens: Mutex<HashMap<String, CancelEntry>>,
    /// Session 内任务入队序号：传输名额 FIFO 调度依据
    enqueue_seq: AtomicU64,
}

/// 活跃任务的取消令牌与 Session 内入队序号：入队序号唯一标识传输池等待队列
/// 中的等待者，取消时据此精确移除，不扰动其余任务的 FIFO 顺序。
struct CancelEntry {
    token: CancelToken,
    queue_seq: u64,
}

impl SftpHandle {
    /// 登记任务取消条目并分配 Session 内入队序号：序号决定传输名额的 FIFO 顺序，
    /// 也是取消时把等待者移出传输池队列的依据。返回入队序号。
    fn register_cancel_entry(&self, task_id: String, token: CancelToken) -> u64 {
        let queue_seq = self.enqueue_seq.fetch_add(1, Ordering::Relaxed);
        self.cancel_tokens
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(task_id, CancelEntry { token, queue_seq });
        queue_seq
    }

    /// 读取任务的取消条目副本（令牌与入队序号），随后立即释放锁。
    fn cancel_entry(&self, task_id: &str) -> Option<(CancelToken, u64)> {
        self.cancel_tokens
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(task_id)
            .map(|entry| (entry.token.clone(), entry.queue_seq))
    }
}

/// File Transfer module，registry 锁不会跨远程 IO seam。
#[derive(Clone)]
pub struct SftpService {
    handles: Arc<Mutex<HashMap<String, Arc<SftpHandle>>>>,
    tasks: Arc<Mutex<HashMap<String, TransferTask>>>,
    connector: SftpConnector,
    /// 传输连接池的单调时间源
    clock: Arc<TransferClock>,
    /// 额外传输连接空闲回收阈值
    idle_timeout: Duration,
    /// 全局并发信号量；测试服务持有独立实例，避免跨测试互相占用 permit
    semaphore: Arc<Semaphore>,
}

impl SftpService {
    /// 创建使用真实 SSH transport adapter 的 File Transfer module。
    /// 生产服务共享全局并发信号量：跨所有 Session 合计最多 5 个并发传输。
    pub fn new() -> Self {
        Self::with_connector_semaphore(
            |host, _role, verifier| connect_sftp_for_host(host, verifier),
            TransferClock::system(),
            TRANSFER_IDLE_TIMEOUT,
            get_semaphore(),
        )
    }

    /// 注入内部连接 adapter，供 transport contract 测试使用；
    /// 测试服务持有独立全局信号量，不与其他测试互相占用 permit。
    #[cfg(test)]
    pub(crate) fn with_connector(
        connector: impl Fn(&HostConfig, SftpRole) -> Result<SftpTransport, AppError>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        Self::with_connector_clock_timeout(
            connector,
            TransferClock::system(),
            TRANSFER_IDLE_TIMEOUT,
        )
    }

    /// 注入携带主机身份校验器的三参连接 adapter 与时间源，
    /// 供验证统一校验不绕过 SFTP 的测试使用。
    #[cfg(test)]
    pub(crate) fn with_verifying_connector(
        connector: impl Fn(&HostConfig, SftpRole, &HostKeyVerifier) -> Result<SftpTransport, AppError>
        + Send
        + Sync
        + 'static,
        clock: TransferClock,
        idle_timeout: Duration,
    ) -> Self {
        Self::with_connector_semaphore(
            connector,
            clock,
            idle_timeout,
            Arc::new(Semaphore::new(MAX_CONCURRENT_TRANSFERS)),
        )
    }

    /// 注入连接 adapter、时间源与空闲回收阈值，供传输连接池 contract 测试使用。
    /// 每个测试服务持有独立全局信号量：并发测试互不占用 permit。
    #[cfg(test)]
    pub(crate) fn with_connector_clock_timeout(
        connector: impl Fn(&HostConfig, SftpRole) -> Result<SftpTransport, AppError>
        + Send
        + Sync
        + 'static,
        clock: TransferClock,
        idle_timeout: Duration,
    ) -> Self {
        Self::with_connector_semaphore(
            move |host, role, _verifier| connector(host, role),
            clock,
            idle_timeout,
            Arc::new(Semaphore::new(MAX_CONCURRENT_TRANSFERS)),
        )
    }

    /// 装配 File Transfer module 的完整构造入口。
    fn with_connector_semaphore(
        connector: impl Fn(&HostConfig, SftpRole, &HostKeyVerifier) -> Result<SftpTransport, AppError>
        + Send
        + Sync
        + 'static,
        clock: TransferClock,
        idle_timeout: Duration,
        semaphore: Arc<Semaphore>,
    ) -> Self {
        Self {
            handles: Arc::new(Mutex::new(HashMap::new())),
            tasks: Arc::new(Mutex::new(HashMap::new())),
            connector: Arc::new(connector),
            clock: Arc::new(clock),
            idle_timeout,
            semaphore,
        }
    }

    /// 注册 Session：并行启动独立控制连接；传输连接池零连接起步，
    /// 基础一条在首次传输时按需建立，不预建五条。
    pub fn register_session_with_verifier(
        &self,
        session_id: String,
        host: HostConfig,
        verifier: HostKeyVerifier,
    ) {
        let control = Arc::new(SftpConnection::new(
            host.clone(),
            verifier.clone(),
            self.connector.clone(),
            SftpRole::Control,
        ));
        let transfer_pool = TransferPool::new_cyclic(
            host,
            verifier,
            self.connector.clone(),
            self.clock.clone(),
            self.idle_timeout,
        );
        self.handles
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(
                session_id,
                Arc::new(SftpHandle {
                    control: control.clone(),
                    transfer_pool,
                    cancel_tokens: Mutex::new(HashMap::new()),
                    enqueue_seq: AtomicU64::new(0),
                }),
            );
        control.connect_eager();
    }

    /// 测试便捷入口：以总是放行的校验器注册 Session（生产必须走 register_session_with_verifier）。
    #[cfg(test)]
    pub(crate) fn register_session(&self, session_id: String, host: HostConfig) {
        self.register_session_with_verifier(session_id, host, test_allow_all_verifier());
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
            .ok_or_else(|| {
                AppError::SftpChannelError(ErrorDetail::msg(
                    "session {0} 不存在",
                    vec![session_id.to_string()],
                ))
            })
    }

    /// 列举远程目录内容，按目录优先、名称排序
    ///
    /// 操作发现 Ready 控制连接失效时自动淘汰并重连一次；第二次失败原样返回结构化错误。
    ///
    /// # 参数
    /// - `session_id`: 关联的 SSH 会话 ID
    /// - `path`: 远程目录绝对路径
    ///
    /// # 返回
    /// 成功返回 RemoteEntry 列表，失败返回 AppError
    pub fn list_dir(&self, session_id: &str, path: &str) -> Result<Vec<RemoteEntry>, AppError> {
        let entries = self.run_control_op(session_id, |sftp| sftp.list_dir(path))?;
        let mut entries: Vec<RemoteEntry> = entries
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

    /// 在专用控制连接上持锁执行一次目录/元数据操作；发现失效连接时淘汰并自动重连一次。
    ///
    /// # 参数
    /// - `session_id`: 关联会话 ID
    /// - `op`: 在控制连接上执行的操作（目录列举、远端文件大小查询等只读操作）
    ///
    /// # 返回
    /// 成功返回操作结果，失败返回结构化错误。
    /// 域错误（路径不存在、权限拒绝）不触发重连；连接类错误只重试一次，
    /// 第二次失败（含重连失败）原样返回，不无限重试。
    fn run_control_op<T>(
        &self,
        session_id: &str,
        op: impl Fn(&mut SftpTransport) -> Result<T, AppError>,
    ) -> Result<T, AppError> {
        let handle = self.handle(session_id)?;
        let transport = handle.control.get()?;
        match run_op_locked(&transport, &op) {
            Ok(value) => Ok(value),
            Err(error) if is_connection_failure(&error) => {
                // 淘汰本次操作实际使用的失效连接并自动重连一次；第二次失败原样返回。
                // 只淘汰该连接本身：并发操作可能已重建新连接，不得误淘汰健康连接。
                handle.control.invalidate_if_ready(&transport);
                let transport = handle.control.get()?;
                run_op_locked(&transport, &op)
            }
            Err(error) => Err(error),
        }
    }

    /// 发起下载任务，立即返回 status = Pending 的 TransferTask
    ///
    /// 冲突策略缺省 Reject：目标已存在时任务在发布阶段失败，绝不覆盖本地文件；
    /// Overwrite 仅在用户逐文件确认后使用。
    ///
    /// # 参数
    /// - `session_id`: 关联会话 ID
    /// - `remote_path`: 远程文件完整路径
    /// - `local_path`: 本地保存路径（父目录必须存在）
    /// - `conflict_strategy`: 目标已存在时的处理策略（Reject / Overwrite）
    /// - `app`: Tauri 应用句柄，用于推送事件
    pub fn enqueue_download<R: Runtime>(
        &self,
        session_id: String,
        remote_path: String,
        local_path: String,
        conflict_strategy: ConflictStrategy,
        app: AppHandle<R>,
    ) -> Result<TransferTask, AppError> {
        // 验证本地路径父目录可写
        let parent = Path::new(&local_path).parent().ok_or_else(|| {
            AppError::SftpTransferError(ErrorDetail::msg("本地路径无效", Vec::new()))
        })?;
        if !parent.exists() {
            return Err(AppError::SftpTransferError(ErrorDetail::msg(
                "本地目录不存在: {0}",
                vec![parent.display().to_string()],
            )));
        }
        // 最终目标必须包含文件名：临时文件以目标文件名为基、与目标同目录，
        // 无法满足时宁可拒绝也不降级到其他目录
        if Path::new(&local_path).file_name().is_none() {
            return Err(AppError::SftpTransferError(ErrorDetail::msg(
                "本地路径无效",
                Vec::new(),
            )));
        }

        let handle = self.handle(&session_id)?;

        // 同一 Session 已有 Pending/Running 下载占用相同最终目标时拒绝入队：
        // 并发写同一本地目标会互相破坏临时文件与发布语义
        {
            let tasks = self
                .tasks
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let occupied = tasks.values().any(|task| {
                task.session_id == session_id
                    && task.transfer_type == TransferType::Download
                    && matches!(
                        task.status,
                        SftpTaskStatus::Pending | SftpTaskStatus::Running
                    )
                    && task.local_path == local_path
            });
            if occupied {
                return Err(AppError::SftpTargetBusy(local_path.into()));
            }
        }

        let file_name = Path::new(&remote_path)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| remote_path.clone());

        // 元数据操作复用控制连接：失效时淘汰并自动重连一次，第二次失败原样返回
        let total_bytes = self.run_control_op(&session_id, |sftp| sftp.file_size(&remote_path))?;

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
            error: None,
            created_at: chrono::Utc::now().timestamp_millis(),
        };

        self.tasks
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(task_id.clone(), task.clone());
        // 入队序号决定 Session 内传输名额的 FIFO 顺序，也是取消时移出队列的依据
        let queue_seq = handle.register_cancel_entry(task_id.clone(), cancel_token.clone());

        self.spawn_transfer_task(
            queue_seq,
            task_id,
            session_id,
            remote_path,
            local_path,
            total_bytes,
            TransferType::Download,
            Some(conflict_strategy),
            cancel_token,
            app,
        );

        Ok(task)
    }

    /// 发起上传任务，立即返回 status = Pending 的 TransferTask
    ///
    /// 冲突策略缺省 Reject：远端目标已存在时任务在发布阶段失败，绝不覆盖远端文件；
    /// Overwrite 仅在用户逐文件确认后使用，经同目录远端临时文件安全发布。
    ///
    /// # 参数
    /// - `session_id`: 关联会话 ID
    /// - `local_path`: 本地文件完整路径
    /// - `remote_path`: 远程目标目录路径（后端自动拼接文件名）
    /// - `conflict_strategy`: 目标已存在时的处理策略（Reject / Overwrite）
    /// - `app`: Tauri 应用句柄，用于推送事件
    pub fn enqueue_upload<R: Runtime>(
        &self,
        session_id: String,
        local_path: String,
        remote_path: String,
        conflict_strategy: ConflictStrategy,
        app: AppHandle<R>,
    ) -> Result<TransferTask, AppError> {
        // 验证本地文件存在
        if !Path::new(&local_path).exists() {
            return Err(AppError::SftpTransferError(ErrorDetail::msg(
                "本地文件不存在: {0}",
                vec![local_path],
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

        // 传输连接由 worker 按需建立：命令线程不被远端建连阻塞
        let handle = self.handle(&session_id)?;

        // 同一 Session 已有 Pending/Running 上传占用相同最终目标时拒绝入队：
        // 并发写同一远端目标会互相破坏临时文件与安全发布语义
        {
            let tasks = self
                .tasks
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let occupied = tasks.values().any(|task| {
                task.session_id == session_id
                    && task.transfer_type == TransferType::Upload
                    && matches!(
                        task.status,
                        SftpTaskStatus::Pending | SftpTaskStatus::Running
                    )
                    && task.remote_path == full_remote_path
            });
            if occupied {
                return Err(AppError::SftpTargetBusy(full_remote_path.into()));
            }
        }

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
            error: None,
            created_at: chrono::Utc::now().timestamp_millis(),
        };

        self.tasks
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(task_id.clone(), task.clone());
        // 入队序号决定 Session 内传输名额的 FIFO 顺序，也是取消时移出队列的依据
        let queue_seq = handle.register_cancel_entry(task_id.clone(), cancel_token.clone());
        self.spawn_transfer_task(
            queue_seq,
            task_id,
            session_id,
            full_remote_path,
            local_path,
            total_bytes,
            TransferType::Upload,
            Some(conflict_strategy),
            cancel_token,
            app,
        );

        Ok(task)
    }

    /// 取消指定传输任务；任务不存在时返回结构化错误，已终态任务静默成功
    ///
    /// # 参数
    /// - `task_id`: 要取消的任务 ID
    ///
    /// # 返回
    /// Ok(()) 表示取消令牌已触发或任务已为终态；Err(SftpTaskNotFound) 表示
    /// 任务从未入队（取消失败必须对用户可见，不再静默吞掉）
    pub fn cancel_task(&self, task_id: &str) -> Result<(), AppError> {
        // 找到对应 session 的取消令牌并触发取消
        let handles: Vec<_> = self
            .handles
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .values()
            .cloned()
            .collect();
        for handle in handles {
            if let Some((token, queue_seq)) = handle.cancel_entry(task_id) {
                // 先触发取消令牌：任何阶段的 worker 都能感知；
                // 再把等待者移出 Session FIFO 队列并唤醒：Pending 任务立即迁移
                // Cancelled，其余等待者的 FIFO 顺序不受影响
                token.cancel();
                handle.transfer_pool.cancel_waiter(queue_seq);
                return Ok(());
            }
        }
        // 令牌缺失：registry 为权威状态，区分“已终态”与“任务不存在”
        let task = self
            .tasks
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(task_id)
            .cloned();
        match task {
            Some(task) if is_terminal(&task.status) => Ok(()),
            _ => Err(AppError::SftpTaskNotFound(task_id.to_string().into())),
        }
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
    /// - `error`: 结构化失败原因；Failed 或取消后清理失败时为具体应用错误，
    ///   其余为 None，registry 与事件 payload 各写入一份相同副本
    ///
    /// # 返回
    /// true 表示迁移成功且已发布事件；false 表示被拒绝（未知任务或非法迁移）
    fn transition_task<R: Runtime>(
        &self,
        app: &AppHandle<R>,
        task_id: &str,
        session_id: &str,
        status: SftpTaskStatus,
        error: Option<AppErrorInfo>,
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
        task.error = error.clone();
        // 终态迁移在 registry 同一临界区内执行淘汰：不暴露超限中间状态
        if is_terminal(&status) {
            evict_old_terminal_tasks(&mut tasks, session_id);
        }
        drop(tasks);

        // 终态后移除取消令牌；session 已关闭时（cleanup 后）跳过
        if is_terminal(&status)
            && let Ok(handle) = self.handle(session_id)
        {
            handle
                .cancel_tokens
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .remove(task_id);
        }

        let _ = app.emit(
            "sftp:task_status",
            SftpTaskStatusEvent {
                task_id: task_id.to_string(),
                session_id: session_id.to_string(),
                status,
                error,
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
            // 同时关闭控制 capability 与传输连接池，丢弃任何迟到连接结果；
            // 池关闭唤醒全部等待 checkout 的 worker，其终态迁移因任务已移除被拒绝
            handle.control.close();
            handle.transfer_pool.close();
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
                    .map(|(task_id, entry)| (task_id.clone(), entry.token.clone()))
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

    /// 返回指定 Session 的权威任务快照，按 createdAt 最新优先排序。
    ///
    /// 前端用快照重建投影以恢复错过的事件，后续事件继续增量更新；
    /// Session 关闭后 registry 已整体清空，快照返回空列表。
    ///
    /// # 参数
    /// - `session_id`: 关联会话 ID
    pub fn task_snapshot(&self, session_id: &str) -> Vec<TransferTask> {
        let mut tasks: Vec<TransferTask> = self
            .tasks
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .values()
            .filter(|task| task.session_id == session_id)
            .cloned()
            .collect();
        tasks.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        tasks
    }

    /// 清除指定 Session 的全部终态记录（Done/Failed/Cancelled）。
    ///
    /// Pending/Running 活动任务与其他 Session 的记录不受影响；Session 不存在或
    /// 无终态任务时静默成功（幂等），不产生失败路径。
    ///
    /// # 参数
    /// - `session_id`: 关联会话 ID
    pub fn clear_terminal_tasks(&self, session_id: &str) {
        self.tasks
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .retain(|_, task| task.session_id != session_id || !is_terminal(&task.status));
    }

    /// 在独立 tokio task 中执行传输：先取得 Session 传输名额（按需建连或 FIFO 等待），
    /// 再竞争全局 permit，通过 transition 更新 registry 并推送状态事件。
    ///
    /// 传输连接在阻塞线程内按需建立：命令线程与 Session 打开不被远端 IO 阻塞；
    /// 控制连接不参与传输，目录/元数据操作在传输期间保持响应。
    /// 等待 Session 名额期间任务保持 Pending，不占用全局 permit。
    ///
    /// # 参数
    /// - `queue_seq`: Session 内入队序号，决定传输名额的 FIFO 顺序
    /// - `task_id`: 任务唯一 ID
    /// - `session_id`: 关联会话 ID
    /// - `remote_path`: 远程文件路径
    /// - `local_path`: 本地文件路径
    /// - `total_bytes`: 文件总大小
    /// - `transfer_type`: 传输方向
    /// - `conflict_strategy`: 上传/下载共用的冲突策略（Reject / Overwrite）
    /// - `cancel_token`: 取消令牌
    /// - `app`: Tauri 应用句柄
    fn spawn_transfer_task<R: Runtime + 'static>(
        &self,
        queue_seq: u64,
        task_id: String,
        session_id: String,
        remote_path: String,
        local_path: String,
        total_bytes: u64,
        transfer_type: TransferType,
        conflict_strategy: Option<ConflictStrategy>,
        cancel_token: CancelToken,
        app: AppHandle<R>,
    ) {
        let semaphore = self.semaphore.clone();
        let service = self.clone();
        // 用 tauri 的 async_runtime 而非裸 tokio::spawn：同步 Tauri command 线程没有
        // reactor 上下文，裸 spawn 会 panic；async_runtime 无全局 runtime 时自动回退到独立线程 runtime
        tauri::async_runtime::spawn(async move {
            // ① 先取得 Session 传输名额：阻塞 checkout 在阻塞线程内按需建连或
            //    FIFO 等待释放，等待期间任务保持 Pending，不占用全局 permit
            let checkout_result = {
                let service = service.clone();
                let session_id = session_id.clone();
                tokio::task::spawn_blocking(move || match service.handle(&session_id) {
                    Ok(handle) => handle.transfer_pool.checkout(queue_seq),
                    Err(error) => Err(CheckoutError::Connect(error)),
                })
                .await
            };
            let checkout = match checkout_result {
                Ok(Ok(checkout)) => checkout,
                Ok(Err(CheckoutError::Cancelled)) => {
                    // 排队期间被取消：已移出 Session 队列，Pending → Cancelled 为合法迁移
                    service.transition_task(
                        &app,
                        &task_id,
                        &session_id,
                        SftpTaskStatus::Cancelled,
                        None,
                    );
                    return;
                }
                Ok(Err(CheckoutError::Closed)) => {
                    // Session 已关闭：cleanup 已迁移任务并整体移除 registry，
                    // 迟到 worker 静默终止，不重复发事件
                    return;
                }
                Ok(Err(CheckoutError::Connect(error))) => {
                    // 建连失败只影响本任务，保留结构化错误。
                    // 状态机只允许 Pending → Running → Failed：先进入 Running 再迁移到
                    // Failed，保证每步都是合法迁移。
                    service.transition_task(
                        &app,
                        &task_id,
                        &session_id,
                        SftpTaskStatus::Running,
                        None,
                    );
                    service.transition_task(
                        &app,
                        &task_id,
                        &session_id,
                        SftpTaskStatus::Failed,
                        Some(AppErrorInfo::from(error)),
                    );
                    return;
                }
                Err(join_error) => {
                    service.transition_task(
                        &app,
                        &task_id,
                        &session_id,
                        SftpTaskStatus::Running,
                        None,
                    );
                    service.transition_task(
                        &app,
                        &task_id,
                        &session_id,
                        SftpTaskStatus::Failed,
                        Some(AppErrorInfo::from(AppError::SftpTransferError(
                            join_error.to_string().into(),
                        ))),
                    );
                    return;
                }
            };

            // ② 再竞争全局并发 permit（跨 Session 上限，tokio 信号量 FIFO 公平）；
            //    等待期间被取消则立即归还 Session 名额并迁移 Cancelled，不占住名额
            let _permit = tokio::select! {
                permit = semaphore.acquire() => match permit {
                    // 信号量从不关闭；异常关闭时按结构化失败迁移，不 panic
                    Ok(permit) => permit,
                    Err(_) => {
                        service.release_transfer_connection(&session_id, checkout, true);
                        service.transition_task(
                            &app,
                            &task_id,
                            &session_id,
                            SftpTaskStatus::Running,
                            None,
                        );
                        service.transition_task(
                            &app,
                            &task_id,
                            &session_id,
                            SftpTaskStatus::Failed,
                            Some(AppErrorInfo::from(AppError::SftpTransferError(
                                ErrorDetail::msg("全局传输信号量已关闭", Vec::new()),
                            ))),
                        );
                        return;
                    }
                },
                _ = cancel_token.cancelled() => {
                    service.release_transfer_connection(&session_id, checkout, true);
                    service.transition_task(
                        &app,
                        &task_id,
                        &session_id,
                        SftpTaskStatus::Cancelled,
                        None,
                    );
                    return;
                }
            };

            // ③ select 随机分支可能已错过并发取消：迁移 Running 前再确认一次；
            //    若已由 cleanup 迁移则被拒绝
            if cancel_token.is_cancelled() {
                service.release_transfer_connection(&session_id, checkout, true);
                service.transition_task(
                    &app,
                    &task_id,
                    &session_id,
                    SftpTaskStatus::Cancelled,
                    None,
                );
                return;
            }

            // ④ 迁移到 Running 后阻塞执行传输
            service.transition_task(&app, &task_id, &session_id, SftpTaskStatus::Running, None);

            let task_id_clone = task_id.clone();
            let session_id_clone = session_id.clone();
            let app_clone = app.clone();
            let cancel_token_clone = cancel_token.clone();
            let transfer_type_clone = transfer_type.clone();
            let transport = checkout.transport.clone();

            let result = tokio::task::spawn_blocking(move || {
                run_transfer_blocking(
                    &task_id_clone,
                    &session_id_clone,
                    &remote_path,
                    &local_path,
                    total_bytes,
                    &transfer_type_clone,
                    conflict_strategy,
                    &transport,
                    &cancel_token_clone,
                    &app_clone,
                )
            })
            .await;

            // ⑤ 归还传输连接：连接类失败直接淘汰，其余回到池中等待复用或超时回收
            let (outcome, healthy) = match result {
                Ok(outcome) => {
                    let healthy = !matches!(
                        &outcome,
                        TransferOutcome::Failed(error) if is_connection_failure(error)
                    );
                    (outcome, healthy)
                }
                Err(join_error) => (
                    TransferOutcome::Failed(AppError::SftpTransferError(
                        join_error.to_string().into(),
                    )),
                    false,
                ),
            };
            service.release_transfer_connection(&session_id, checkout, healthy);

            // ⑥ 终态迁移（registry 先更新再发事件）
            match outcome {
                TransferOutcome::Done => {
                    service.transition_task(
                        &app,
                        &task_id,
                        &session_id,
                        SftpTaskStatus::Done,
                        None,
                    );
                }
                TransferOutcome::Cancelled(cleanup_error) => {
                    service.transition_task(
                        &app,
                        &task_id,
                        &session_id,
                        SftpTaskStatus::Cancelled,
                        cleanup_error.map(AppErrorInfo::from),
                    );
                }
                TransferOutcome::Failed(error) => {
                    // 本次任务保留结构化错误，不自动重跑（传输开始后不重试）；
                    // 失效连接已在 ⑤ 淘汰，下一次传输自动重建
                    service.transition_task(
                        &app,
                        &task_id,
                        &session_id,
                        SftpTaskStatus::Failed,
                        Some(AppErrorInfo::from(error)),
                    );
                }
            }
        });
    }

    /// 归还传输连接：失效连接直接淘汰，健康连接回到池中等待复用或超时回收。
    fn release_transfer_connection(
        &self,
        session_id: &str,
        checkout: TransferCheckout,
        healthy: bool,
    ) {
        if let Ok(handle) = self.handle(session_id) {
            handle.transfer_pool.checkin(checkout, healthy);
        }
        // Session 已关闭：checkout 随本函数 drop 释放 capability
    }
}

/// 判断任务状态是否为终态（不再接受任何迁移）
fn is_terminal(status: &SftpTaskStatus) -> bool {
    matches!(
        status,
        SftpTaskStatus::Done | SftpTaskStatus::Failed | SftpTaskStatus::Cancelled
    )
}

/// 淘汰指定 Session 超出上限的最旧终态任务；Pending/Running 永不淘汰。
///
/// 只在终态迁移后的同一 registry 临界区内调用：淘汰与迁移原子完成，
/// 快照和并发 worker 观察不到超过 MAX_TERMINAL_TASKS_PER_SESSION 的中间状态。
/// 相同 createdAt 时按 task_id 降序破平：UUID 唯一且不随时间变化，淘汰结果跨运行确定。
fn evict_old_terminal_tasks(tasks: &mut HashMap<String, TransferTask>, session_id: &str) {
    let mut terminal: Vec<(i64, String)> = tasks
        .iter()
        .filter(|(_, task)| task.session_id == session_id && is_terminal(&task.status))
        .map(|(task_id, task)| (task.created_at, task_id.clone()))
        .collect();
    if terminal.len() <= MAX_TERMINAL_TASKS_PER_SESSION {
        return;
    }
    terminal.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1))); // 最新优先，task_id 破平
    for (_, task_id) in terminal.into_iter().skip(MAX_TERMINAL_TASKS_PER_SESSION) {
        tasks.remove(&task_id);
    }
}

/// 判断操作错误是否为失效连接信号（控制与传输连接共用分类），值得淘汰并重建一次。
///
/// 通道错误来自底层 ssh2 会话级失败（如连接已被服务端断开），连接错误为适配器
/// 上报的连接类失败；域错误（路径不存在、权限拒绝等）说明连接本身健康，不触发重建。
fn is_connection_failure(error: &AppError) -> bool {
    matches!(
        error,
        AppError::SftpChannelError(_) | AppError::SshConnectionError(_)
    )
}

/// 持锁执行一次 SFTP 操作；锁中毒保持结构化通道错误语义，不触发重连。
fn run_op_locked<T>(
    transport: &Arc<Mutex<SftpTransport>>,
    op: &impl Fn(&mut SftpTransport) -> Result<T, AppError>,
) -> Result<T, AppError> {
    let mut sftp = transport
        .lock()
        .map_err(|error| AppError::SftpChannelError(error.to_string().into()))?;
    op(&mut sftp)
}

/// 从 secure storage 读取运行时凭据并建立独立 SFTP transport；
/// 主机身份校验在 transport 握手后、认证前生效。
fn connect_sftp_for_host(
    host: &HostConfig,
    verifier: &HostKeyVerifier,
) -> Result<SftpTransport, AppError> {
    let (password, passphrase) = match host.auth_type {
        AuthType::Password => {
            let password_ref = host.password_ref.as_deref().ok_or_else(|| {
                AppError::InvalidHostConfig(ErrorDetail::msg("密码引用为空", Vec::new()))
            })?;
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
    ssh_transport::connect_sftp(host, password.as_deref(), passphrase.as_deref(), verifier)
}

/// 构建总是放行的主机身份校验器，仅供测试便捷入口使用。
#[cfg(test)]
fn test_allow_all_verifier() -> HostKeyVerifier {
    Arc::new(|_presented| Ok(()))
}

/// 传输 worker 的终态结果；失败携带具体 AppError，供淘汰失效传输连接与事件序列化。
/// Cancelled 携带可选的临时文件清理错误：清理失败时错误必须包含临时路径。
#[derive(Debug)]
enum TransferOutcome {
    Done,
    Cancelled(Option<AppError>),
    Failed(AppError),
}

/// 阻塞执行实际传输，每 500ms 推送进度；各阶段失败保留具体结构化错误。
///
/// # 返回
/// - `Done`: 传输成功
/// - `Cancelled`: 主动取消（含残留文件清理）
/// - `Failed(error)`: 打开/读取/写入/创建任一阶段失败，携带阶段对应的应用错误
fn run_transfer_blocking<R: Runtime>(
    task_id: &str,
    session_id: &str,
    remote_path: &str,
    local_path: &str,
    total_bytes: u64,
    transfer_type: &TransferType,
    conflict_strategy: Option<ConflictStrategy>,
    transport: &Arc<Mutex<SftpTransport>>,
    cancel_token: &CancelToken,
    app: &AppHandle<R>,
) -> TransferOutcome {
    use std::io::{Read, Write};
    use std::time::Instant;

    let mut sftp = match transport.lock() {
        Ok(sftp) => sftp,
        Err(error) => {
            return TransferOutcome::Failed(AppError::SftpChannelError(error.to_string().into()));
        }
    };

    const CHUNK: usize = 32 * 1024; // 32KB chunks
    let mut transferred: u64 = 0;
    let mut last_report = Instant::now();
    let mut last_transferred: u64 = 0;
    let mut buf = vec![0u8; CHUNK];

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
            let conflict_strategy = conflict_strategy.unwrap_or_default();
            // 取消检查先于任何本地写入：已取消任务不得创建临时文件
            if cancel_token.is_cancelled() {
                return TransferOutcome::Cancelled(None);
            }
            let mut remote_file = match sftp.open_read(remote_path) {
                Ok(file) => file,
                Err(error) => return TransferOutcome::Failed(error),
            };
            // 与最终目标同目录、包含 taskId 的唯一临时文件：发布前原目标不受影响
            let temp_path = download_temp_path(local_path, task_id);
            let mut local_file = match std::fs::File::create(&temp_path) {
                Ok(file) => file,
                Err(error) => {
                    return TransferOutcome::Failed(AppError::SftpCreateError(
                        format!("{} ({})", temp_path.display(), error).into(),
                    ));
                }
            };

            /// 关闭本地文件句柄并尽力删除临时文件（取消或 IO 失败时调用）
            macro_rules! cleanup_temp {
                () => {{
                    drop(local_file);
                    cleanup_download_temp(&temp_path)
                }};
            }

            loop {
                if cancel_token.is_cancelled() {
                    // 主动取消：先停止 IO、清理临时文件，再返回终态；原文件不受影响
                    return TransferOutcome::Cancelled(cleanup_temp!().err());
                }
                let n = match remote_file.read(&mut buf) {
                    Ok(n) => n,
                    Err(error) => {
                        let primary = AppError::SftpReadError(error.to_string().into());
                        return TransferOutcome::Failed(merge_cleanup_failure(
                            primary,
                            cleanup_temp!(),
                        ));
                    }
                };
                if n == 0 {
                    break;
                }
                if let Err(error) = local_file.write_all(&buf[..n]) {
                    let primary = AppError::SftpWriteError(error.to_string().into());
                    return TransferOutcome::Failed(merge_cleanup_failure(
                        primary,
                        cleanup_temp!(),
                    ));
                }
                transferred += n as u64;
                emit_progress!();
            }

            // 成功刷新并关闭后才允许发布：任何失败都不得触碰最终目标
            if let Err(error) = local_file.flush().and_then(|_| local_file.sync_all()) {
                let primary = AppError::SftpWriteError(error.to_string().into());
                return TransferOutcome::Failed(merge_cleanup_failure(primary, cleanup_temp!()));
            }
            drop(local_file);

            // 发布前再次检查取消：已取消的传输不得发布最终文件
            if cancel_token.is_cancelled() {
                return TransferOutcome::Cancelled(cleanup_download_temp(&temp_path).err());
            }

            // 发布：Reject 先检查目标并 no-clobber 重命名；Overwrite 原子替换。
            // 平台无法保证安全替换时任务失败，原文件保留。
            if let Err(error) =
                publish_download_file(&temp_path, Path::new(local_path), conflict_strategy)
            {
                return TransferOutcome::Failed(merge_cleanup_failure(
                    error,
                    cleanup_download_temp(&temp_path),
                ));
            }
        }
        TransferType::Upload => {
            let conflict_strategy = conflict_strategy.unwrap_or_default();
            // 取消检查先于任何远端写入：已取消任务不得创建临时文件
            if cancel_token.is_cancelled() {
                return TransferOutcome::Cancelled(None);
            }
            let mut local_file = match std::fs::File::open(local_path) {
                Ok(file) => file,
                Err(error) => {
                    return TransferOutcome::Failed(AppError::SftpOpenError(
                        error.to_string().into(),
                    ));
                }
            };
            // 与最终目标同目录、包含 taskId 的唯一远端临时文件：发布前旧目标不受影响
            let temp_path = upload_temp_path(remote_path, task_id);
            let temp_path_str = temp_path.to_string_lossy().to_string();
            let mut remote_file = match sftp.create(&temp_path_str) {
                Ok(file) => file,
                Err(error) => return TransferOutcome::Failed(error),
            };

            /// 关闭远端句柄并尽力删除本任务临时文件（取消或 IO 失败时调用）；
            /// 清理失败的错误并入任务终态，不得静默吞掉
            macro_rules! cleanup_remote_temp {
                () => {{
                    drop(remote_file);
                    cleanup_upload_temp(&mut sftp, &temp_path_str)
                }};
            }

            loop {
                if cancel_token.is_cancelled() {
                    // 主动取消：先停止 IO、清理本任务临时文件，再返回终态；旧目标不受影响
                    return TransferOutcome::Cancelled(cleanup_remote_temp!().err());
                }
                let n = match local_file.read(&mut buf) {
                    Ok(n) => n,
                    Err(error) => {
                        let primary = AppError::SftpReadError(error.to_string().into());
                        return TransferOutcome::Failed(merge_cleanup_failure(
                            primary,
                            cleanup_remote_temp!(),
                        ));
                    }
                };
                if n == 0 {
                    break;
                }
                if let Err(error) = remote_file.write_all(&buf[..n]) {
                    let primary = AppError::SftpWriteError(error.to_string().into());
                    return TransferOutcome::Failed(merge_cleanup_failure(
                        primary,
                        cleanup_remote_temp!(),
                    ));
                }
                transferred += n as u64;
                emit_progress!();
            }

            // 刷新后关闭远端句柄（FXP_CLOSE 使服务器落定已提交写入）才允许发布：
            // 任何失败都不得触碰远端最终目标
            if let Err(error) = remote_file.flush() {
                let primary = AppError::SftpWriteError(error.to_string().into());
                return TransferOutcome::Failed(merge_cleanup_failure(
                    primary,
                    cleanup_remote_temp!(),
                ));
            }
            drop(remote_file);

            // 发布前再次检查取消：已取消的传输不得发布最终文件
            if cancel_token.is_cancelled() {
                return TransferOutcome::Cancelled(
                    cleanup_upload_temp(&mut sftp, &temp_path_str).err(),
                );
            }

            // 安全发布：Reject 拒绝已有目标；Overwrite 要求远端原子替换，
            // 远端无法保证安全替换时保留旧目标并失败，绝不先删旧文件
            if let Err(error) =
                publish_upload_file(&mut sftp, &temp_path_str, remote_path, conflict_strategy)
            {
                return TransferOutcome::Failed(merge_cleanup_failure(
                    error,
                    cleanup_upload_temp(&mut sftp, &temp_path_str),
                ));
            }
        }
    }
    TransferOutcome::Done
}

/// 计算与最终目标同目录、包含 taskId 的下载临时文件路径。
///
/// 命名规则：.文件名.taskId.part；taskId 全局唯一，同目标并发任务也不会撞名，
/// 且与最终目标同目录，保证发布重命名不会跨文件系统。
fn download_temp_path(local_path: &str, task_id: &str) -> PathBuf {
    let target = Path::new(local_path);
    let file_name = target
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default();
    let temp_name = format!(".{}.{}.part", file_name, task_id);
    match target.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.join(temp_name),
        _ => PathBuf::from(temp_name),
    }
}

/// 计算与最终目标同目录、包含 taskId 的上传临时文件路径（远端）。
///
/// 与下载共用同一命名规则（.文件名.taskId.part）：taskId 全局唯一，
/// 同目标并发任务不会撞名；临时文件与最终目标同目录，发布重命名不跨目录。
fn upload_temp_path(remote_path: &str, task_id: &str) -> PathBuf {
    download_temp_path(remote_path, task_id)
}

/// 把已完成写入并关闭句柄的远端临时文件安全发布为最终目标：
///
/// - 目标不存在：no-clobber 重命名；检查与重命名之间竞态新出现的目标同样
///   返回 SftpTargetExists，绝不覆盖未经确认的文件。
/// - Reject + 目标已存在：返回 SftpTargetExists，旧目标保持不动。
/// - Overwrite + 目标已存在：要求远端原子替换；远端无法保证安全替换时
///   保留旧目标并返回 SftpPublishError，绝不先删旧文件。
fn publish_upload_file(
    sftp: &mut SftpTransport,
    temp_path: &str,
    target_path: &str,
    strategy: ConflictStrategy,
) -> Result<(), AppError> {
    // 冲突检查复用传输连接上的元数据操作：路径不存在 → 可发布，其余错误原样传播
    let exists = match sftp.file_size(target_path) {
        Ok(_) => true,
        Err(AppError::SftpPathNotFound(_)) => false,
        Err(error) => return Err(error),
    };
    if exists && strategy == ConflictStrategy::Reject {
        return Err(AppError::SftpTargetExists(target_path.to_string().into()));
    }
    if !exists {
        // 目标不存在：no-clobber 发布；Overwrite 策略下竞态出现的目标同样被拒，
        // 由用户对单个文件重新确认覆盖
        return sftp.rename(temp_path, target_path, false);
    }
    // 目标已存在 + Overwrite：远端原子替换，旧目标保留至替换成功
    sftp.rename(temp_path, target_path, true)
}

/// 尽力删除本任务的上传临时文件；删除失败返回包含临时路径的清理错误。
///
/// 只删除本任务的临时路径，绝不扫描未知 `.part` 文件；清理失败不得被吞掉，
/// 调用方把该错误并入任务终态（Cancelled/Failed 的 error detail）。
fn cleanup_upload_temp(sftp: &mut SftpTransport, temp_path: &str) -> Result<(), AppError> {
    match sftp.unlink(temp_path) {
        Ok(()) => Ok(()),
        Err(error) => Err(AppError::SftpTransferError(ErrorDetail::msg(
            "清理临时文件失败: {0} ({1})",
            vec![temp_path.to_string(), error.to_string()],
        ))),
    }
}

/// 尽力删除下载临时文件；删除失败返回包含临时路径的清理错误。
///
/// 清理失败不得被吞掉：调用方把该错误并入任务终态，错误 detail 必须含临时路径。
fn cleanup_download_temp(temp_path: &Path) -> Result<(), AppError> {
    if !temp_path.exists() {
        return Ok(());
    }
    std::fs::remove_file(temp_path).map_err(|error| {
        AppError::SftpTransferError(ErrorDetail::msg(
            "清理临时文件失败: {0} ({1})",
            vec![temp_path.display().to_string(), error.to_string()],
        ))
    })
}

/// 主错误与临时文件清理结果合并：清理失败时把临时路径诊断追加到 detail。
fn merge_cleanup_failure(primary: AppError, cleanup: Result<(), AppError>) -> AppError {
    match cleanup {
        Ok(()) => primary,
        Err(cleanup_error) => primary.with_appended_detail(&cleanup_error.to_string()),
    }
}

/// 把已完成 flush + sync 的临时文件发布为最终目标，平台语义保证安全：
///
/// - Reject：发布前仍检查目标是否存在，随后 no-clobber 重命名；检查与重命名之间
///   竞态新出现的目标同样返回 SftpTargetExists，绝不覆盖。
/// - Overwrite：原子替换（POSIX rename / Windows MoveFileEx REPLACE_EXISTING），
///   失败不改动原目标；平台无法替换（如目标为目录）时返回 SftpPublishError。
///
/// 发布失败时尽力关闭并删除临时文件；删除失败的信息并入错误 detail。
fn publish_download_file(
    temp_path: &Path,
    target_path: &Path,
    strategy: ConflictStrategy,
) -> Result<(), AppError> {
    if strategy == ConflictStrategy::Reject && target_path.exists() {
        return Err(AppError::SftpTargetExists(
            target_path.display().to_string().into(),
        ));
    }
    let file = std::fs::File::open(temp_path).map_err(|error| {
        AppError::SftpPublishError(ErrorDetail::msg(
            "打开临时文件失败: {0} ({1})",
            vec![temp_path.display().to_string(), error.to_string()],
        ))
    })?;
    let temp_path_owned = TempPath::try_from_path(temp_path.to_path_buf()).map_err(|error| {
        AppError::SftpPublishError(ErrorDetail::msg(
            "登记临时文件失败: {0} ({1})",
            vec![temp_path.display().to_string(), error.to_string()],
        ))
    })?;
    let named = NamedTempFile::from_parts(file, temp_path_owned);
    let result = match strategy {
        ConflictStrategy::Reject => named.persist_noclobber(target_path),
        ConflictStrategy::Overwrite => named.persist(target_path),
    };
    match result {
        Ok(_) => Ok(()),
        Err(persist_error) => {
            let error = persist_error.error;
            let file = persist_error.file;
            // 关闭即尽力删除临时文件；删除失败信息并入错误 detail，
            // 外层 cleanup_download_temp 作为第二次机会（内部删除失败时重试）
            let cleanup_failure = file.close().err().map(|cleanup_error| {
                format!(
                    "清理临时文件失败: {} ({})",
                    temp_path.display(),
                    cleanup_error
                )
            });
            let already_exists = strategy == ConflictStrategy::Reject
                && error.kind() == std::io::ErrorKind::AlreadyExists;
            let app_error = if already_exists {
                AppError::SftpTargetExists(target_path.display().to_string().into())
            } else {
                AppError::SftpPublishError(ErrorDetail::msg(
                    "发布失败: {0} -> {1} ({2})，目标原文件未受影响",
                    vec![
                        temp_path.display().to_string(),
                        target_path.display().to_string(),
                        error.to_string(),
                    ],
                ))
            };
            match cleanup_failure {
                Some(detail) => Err(app_error.with_appended_detail(&detail)),
                None => Err(app_error),
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
#[path = "sftp_service_test.rs"]
mod tests;
