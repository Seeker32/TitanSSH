use crate::core::host_identity::HostKeyVerifier;
use crate::core::monitor_worker;
use crate::errors::app_error::AppErrorInfo;
use crate::errors::app_error::{AppError, ErrorDetail};
use crate::models::host::{AuthType, HostConfig};
use crate::models::monitor::{MonitorSnapshot, TaskInfo, TaskStatus};
use crate::models::session::TaskStatusEvent;
use crate::storage::secure_store;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use tauri::{AppHandle, Emitter, Runtime};
use uuid::Uuid;

/// 监控任务句柄，包含任务元数据和关闭信号
pub(crate) struct MonitorTaskHandle {
    /// 任务基本信息（ID、类型、状态等）
    pub(crate) task_info: TaskInfo,
    /// 关闭标志，设置为 true 时通知工作线程退出
    pub(crate) shutdown: Arc<AtomicBool>,
}

/// 独立监控服务
///
/// 负责管理所有监控任务的生命周期，包括启动、停止和状态查询。
/// 通过 Arc<Mutex<...>> 保证多线程安全访问。
#[derive(Clone)]
pub struct MonitorService {
    /// 活跃监控任务的 HashMap，键为 task_id
    pub(crate) tasks: Arc<Mutex<HashMap<String, MonitorTaskHandle>>>,
    /// 最新监控快照的 HashMap，键为 session_id
    snapshots: Arc<Mutex<HashMap<String, MonitorSnapshot>>>,
}

impl MonitorService {
    /// 创建新的监控服务实例
    pub fn new() -> Self {
        Self {
            tasks: Arc::new(Mutex::new(HashMap::new())),
            snapshots: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// 为指定会话启动监控任务（真实 SSH 采集）
    ///
    /// 生成唯一 task_id，创建 TaskInfo（初始状态为 Pending），
    /// 启动后台工作线程通过独立 SSH 连接定期采集快照并推送事件。
    ///
    /// # 参数
    /// - `session_id`: 关联的会话 ID
    /// - `host`: 主机配置（不含明文凭据）
    /// - `app`: Tauri 应用句柄，用于派发事件
    ///
    /// # 返回
    /// 成功返回新建的 TaskInfo；凭据读取失败时不创建任务
    pub fn start_monitoring<R: Runtime>(
        &self,
        session_id: String,
        host: HostConfig,
        verifier: HostKeyVerifier,
        app: AppHandle<R>,
    ) -> Result<TaskInfo, AppError> {
        // 凭据读取必须先于任务注册，确保失败时不留下幽灵任务或事件。
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

        // 生成唯一任务 ID
        let task_id = Uuid::new_v4().to_string();

        // 创建任务信息，初始状态为 Pending
        let task_info = TaskInfo {
            task_id: task_id.clone(),
            task_type: "monitor".to_string(),
            session_id: Some(session_id.clone()),
            status: TaskStatus::Pending,
            created_at: chrono::Utc::now().timestamp_millis(),
        };

        // 创建关闭标志
        let shutdown = Arc::new(AtomicBool::new(false));

        // 将任务句柄注册到 HashMap
        {
            let mut tasks = lock_unpoisoned(&self.tasks);
            tasks.insert(
                task_id.clone(),
                MonitorTaskHandle {
                    task_info: task_info.clone(),
                    shutdown: shutdown.clone(),
                },
            );
        }

        // 克隆共享状态引用，供工作线程使用
        let tasks_ref = Arc::clone(&self.tasks);
        let snapshots_ref = Arc::clone(&self.snapshots);

        // 派发 Pending 状态事件
        emit_task_status(&app, &task_id, TaskStatus::Pending, None);

        // 启动后台监控工作线程
        thread::spawn(move || {
            // 迁移到 Running：registry 先行更新，事件随后发布
            transition_task_status(&tasks_ref, &app, &task_id, TaskStatus::Running, None);

            let tasks_for_error = Arc::clone(&tasks_ref);
            let app_for_error = app.clone();
            let task_id_for_error = task_id.clone();

            let tasks_for_snap = Arc::clone(&tasks_ref);
            let app_for_snap = app.clone();
            let task_id_for_snap = task_id.clone();
            // 供快照回调在推送失败时终止循环；主句柄随后移入 MonitorLoopParams
            let shutdown_for_snap = shutdown.clone();

            // panic 防护：worker 或任一回调 panic 时迁移 Failed，
            // 任务不得卡死在 Running 的幽灵状态（见 run_loop_with_panic_guard）
            run_loop_with_panic_guard(&tasks_ref, &app, &task_id, move || {
                monitor_worker::run_monitor_loop(
                    verifier,
                    monitor_worker::MonitorLoopParams {
                        host,
                        password,
                        passphrase,
                        session_id,
                        shutdown,
                    },
                    move |snapshot| {
                        // 任务已被 stop 移除时丢弃迟到快照（见 apply_snapshot_if_task_alive）
                        apply_snapshot_if_task_alive(
                            &tasks_for_snap,
                            &snapshots_ref,
                            &app_for_snap,
                            &shutdown_for_snap,
                            &task_id_for_snap,
                            &snapshot,
                        );
                    },
                    move |err| {
                        // 采集失败：迁移任务为 Failed（终态，worker 返回后的 Done 会被拒绝）
                        transition_task_status(
                            &tasks_for_error,
                            &app_for_error,
                            &task_id_for_error,
                            TaskStatus::Failed,
                            monitor_status_error("监控采集失败: {0}", err.to_string()),
                        );
                    },
                );
            });
        });

        Ok(task_info)
    }

    /// 停止指定任务 ID 对应的监控任务
    ///
    /// 设置关闭标志，通知工作线程退出，并从任务 HashMap 中移除句柄。
    /// 移除后 worker 的迟到终态迁移会被拒绝且不发事件，因此终态 Done 事件
    /// 由本方法直接补发：前端不得停留在 Running 显示幽灵任务。
    /// 任务已处终态（Done/Failed，事件已播发过）时不重复补发。
    ///
    /// # 参数
    /// - `app`: Tauri 应用句柄，用于补发终态事件
    /// - `task_id`: 要停止的监控任务 ID
    ///
    /// # 返回
    /// true 表示句柄确实存在并已移除；false 表示任务不存在（从未创建、
    /// 已停止或已过期），调用方可据此区分「已停止」与「早已消失」
    pub fn stop_monitoring<R: Runtime>(&self, app: &AppHandle<R>, task_id: &str) -> bool {
        let mut tasks = lock_unpoisoned(&self.tasks);
        match tasks.remove(task_id) {
            Some(handle) => {
                // 通知工作线程退出
                handle.shutdown.store(true, Ordering::Release);
                let already_terminal = matches!(
                    handle.task_info.status,
                    TaskStatus::Done | TaskStatus::Failed
                );
                drop(tasks);
                if !already_terminal {
                    emit_task_status(app, task_id, TaskStatus::Done, None);
                }
                true
            }
            None => false,
        }
    }

    /// 停止指定会话的全部监控任务；用于后端统一执行 Session teardown。
    ///
    /// 每个被停止且尚未终态的任务补发 Done 终态事件（理由同 stop_monitoring）。
    pub fn stop_session<R: Runtime>(&self, app: &AppHandle<R>, session_id: &str) {
        let mut tasks = lock_unpoisoned(&self.tasks);
        let mut pending_terminal_events: Vec<String> = Vec::new();
        tasks.retain(|task_id, handle| {
            let keep = handle.task_info.session_id.as_deref() != Some(session_id);
            if !keep {
                handle.shutdown.store(true, Ordering::Release);
                let already_terminal = matches!(
                    handle.task_info.status,
                    TaskStatus::Done | TaskStatus::Failed
                );
                if !already_terminal {
                    pending_terminal_events.push(task_id.clone());
                }
            }
            keep
        });
        drop(tasks);
        lock_unpoisoned(&self.snapshots).remove(session_id);
        for task_id in pending_terminal_events {
            emit_task_status(app, &task_id, TaskStatus::Done, None);
        }
    }

    /// 获取指定会话的最新监控快照
    ///
    /// # 参数
    /// - `session_id`: 会话 ID
    ///
    /// # 返回
    /// 若存在缓存快照则返回 Some(MonitorSnapshot)，否则返回 None
    pub fn get_monitor_status(&self, session_id: &str) -> Option<MonitorSnapshot> {
        let snapshots = lock_unpoisoned(&self.snapshots);
        snapshots.get(session_id).cloned()
    }

    /// 测试构造：直接注入快照，供命令层测试有数据路径。
    #[cfg(test)]
    pub(crate) fn insert_snapshot_for_test(&self, snapshot: MonitorSnapshot) {
        lock_unpoisoned(&self.snapshots).insert(snapshot.session_id.clone(), snapshot);
    }
}

/// 应用采集快照：仅当任务仍在 registry（未被 stop 移除）时落缓存并推送事件。
///
/// 持 tasks 锁完成存在性检查与快照写入，与 stop_session 的
/// 「先移除任务、后清理快照」顺序串行化：stop 先持锁则此处检查失败直接
/// 丢弃；本函数先持锁则 stop 的后续清理必然移除本次写入。
/// 在途 collect_once 的迟到快照因此不可能复活已清理的会话数据。
///
/// # 返回
/// true 表示快照已应用（任务存活）；false 表示任务已停止，快照被丢弃
fn apply_snapshot_if_task_alive<R: Runtime>(
    tasks: &Arc<Mutex<HashMap<String, MonitorTaskHandle>>>,
    snapshots: &Arc<Mutex<HashMap<String, MonitorSnapshot>>>,
    app: &AppHandle<R>,
    shutdown: &AtomicBool,
    task_id: &str,
    snapshot: &MonitorSnapshot,
) -> bool {
    {
        let tasks_guard = lock_unpoisoned(tasks);
        if !tasks_guard.contains_key(task_id) {
            return false;
        }
        // 持 tasks 锁写入快照（锁在事件推送前释放，避免与事件回调互相等待）
        lock_unpoisoned(snapshots).insert(snapshot.session_id.clone(), snapshot.clone());
    }
    // 推送事件到前端；失败则任务进入 Failed 终态并终止采集循环
    if let Err(err) = app.emit("monitor:snapshot", snapshot) {
        handle_snapshot_emit_failure(shutdown, tasks, app, task_id, err);
    }
    true
}

/// 处理 monitor:snapshot 推送失败：先设置关闭标志终止采集循环，再把任务
/// 迁移为 Failed（终态事件只发一次）。
///
/// 仅迁移失败而不设关闭标志时，run_monitor_loop 每 2 秒继续采集并重复
/// 失败推送（SSH 连接、远端脚本执行、缓存写入永不停止），直到外部 stop。
fn handle_snapshot_emit_failure<R: Runtime>(
    shutdown: &AtomicBool,
    tasks: &Arc<Mutex<HashMap<String, MonitorTaskHandle>>>,
    app: &AppHandle<R>,
    task_id: &str,
    err: impl std::fmt::Display,
) {
    shutdown.store(true, Ordering::Release);
    transition_task_status(
        tasks,
        app,
        task_id,
        TaskStatus::Failed,
        monitor_status_error("监控快照推送失败: {0}", err.to_string()),
    );
}

/// 毒化容忍锁：持锁线程 panic 后恢复内部值继续服务。
///
/// 任务注册表与快照缓存都是自洽的可替换状态，无跨调用不变量；
/// 一次 panic 后让全部会话的后续监控调用跟着 panic（不可恢复）比
/// 继续服务更糟。
fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 运行监控循环的 panic 防护：正常退出迁移 Done；worker 或任一回调 panic
/// 时迁移 Failed（结构化 MonitorError，携带 panic 文本）。
///
/// 无防护时线程随 panic 死亡，任务永远卡在 Running：无人再发终态事件，
/// 句柄留在 registry，快照缓存停止更新（幽灵任务）。
/// 任务已被 stop 移除时两种迁移都会被拒绝且不发事件（终态事件已由停止方补发）。
fn run_loop_with_panic_guard<R: Runtime>(
    tasks: &Arc<Mutex<HashMap<String, MonitorTaskHandle>>>,
    app: &AppHandle<R>,
    task_id: &str,
    body: impl FnOnce(),
) {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(body)) {
        Ok(()) => {
            // 循环退出时迁移为 Done；若已 Failed 或已被 stop 移除，迁移被拒绝且不发事件
            transition_task_status(tasks, app, task_id, TaskStatus::Done, None);
        }
        Err(payload) => {
            transition_task_status(
                tasks,
                app,
                task_id,
                TaskStatus::Failed,
                monitor_status_error("监控工作线程异常退出: {0}", panic_message(&*payload)),
            );
        }
    }
}

/// 从 catch_unwind 的 payload 提取 panic 信息文本。
fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(text) = payload.downcast_ref::<&str>() {
        (*text).to_string()
    } else if let Some(text) = payload.downcast_ref::<String>() {
        text.clone()
    } else {
        "unknown panic".to_string()
    }
}

/// 迁移任务状态：registry 先更新，再发布事件；任务不存在或迁移非法时拒绝。
///
/// 状态机：Pending → Running → {Done, Failed}；Failed / Done 为终态，
/// 已停止（从 registry 移除）的任务拒绝一切后续迁移。
///
/// # 参数
/// - `tasks`: 任务 registry 的共享引用
/// - `app`: Tauri 应用句柄（泛型，支持真实运行时和测试 MockRuntime）
/// - `task_id`: 任务 ID
/// - `status`: 目标状态
/// - `message`: 可选的附加消息（如错误详情）
///
/// # 返回
/// true 表示迁移成功且已发布事件；false 表示被拒绝（未知任务或非法迁移）
fn transition_task_status<R: Runtime>(
    tasks: &Arc<Mutex<HashMap<String, MonitorTaskHandle>>>,
    app: &AppHandle<R>,
    task_id: &str,
    status: TaskStatus,
    message: Option<AppErrorInfo>,
) -> bool {
    let mut tasks = lock_unpoisoned(tasks);
    let Some(handle) = tasks.get_mut(task_id) else {
        return false;
    };
    let legal = matches!(
        (&handle.task_info.status, &status),
        (TaskStatus::Pending, TaskStatus::Running)
            | (TaskStatus::Running, TaskStatus::Done)
            | (TaskStatus::Running, TaskStatus::Failed)
    );
    if !legal {
        return false;
    }
    handle.task_info.status = status.clone();
    drop(tasks);

    emit_task_status(app, task_id, status, message);
    true
}

/// 派发任务状态变更事件到前端
///
/// # 参数
/// - `app`: Tauri 应用句柄（泛型，支持真实运行时和测试 MockRuntime）
/// - `task_id`: 任务 ID
/// - `status`: 新的任务状态
/// - `message`: 可选的结构化错误（code 稳定，固定文案可翻译）
fn emit_task_status<R: Runtime>(
    app: &AppHandle<R>,
    task_id: &str,
    status: TaskStatus,
    message: Option<AppErrorInfo>,
) {
    let _ = app.emit(
        "task:status",
        TaskStatusEvent {
            task_id: task_id.to_string(),
            status,
            error: message,
        },
    );
}

/// 构建监控任务的结构化错误：固定文案为中文源文案（前端按语言翻译），
/// 参数为语言无关的底层错误文本。
fn monitor_status_error(key: &str, param: String) -> Option<AppErrorInfo> {
    Some(AppErrorInfo {
        code: "MonitorError".to_string(),
        detail: None,
        detail_key: Some(key.to_string()),
        detail_params: Some(vec![param]),
    })
}

#[cfg(test)]
#[path = "monitor_service_test.rs"]
mod service_tests;
