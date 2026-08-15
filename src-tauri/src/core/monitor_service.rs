use crate::core::host_identity::HostKeyVerifier;
use crate::core::monitor_worker;
use crate::errors::app_error::AppError;
use crate::errors::app_error::AppErrorInfo;
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
            let mut tasks = self.tasks.lock().unwrap();
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
            let session_id_for_snap = session_id.clone();

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
                    // 更新快照缓存
                    {
                        let mut snapshots = snapshots_ref.lock().unwrap();
                        snapshots.insert(session_id_for_snap.clone(), snapshot.clone());
                    }
                    // 推送事件到前端，失败则迁移任务为 Failed（Failed 为终态，只迁移一次）
                    if let Err(err) = app_for_snap.emit("monitor:snapshot", &snapshot) {
                        transition_task_status(
                            &tasks_for_snap,
                            &app_for_snap,
                            &task_id_for_snap,
                            TaskStatus::Failed,
                            Some(format!("监控快照推送失败: {err}")),
                        );
                    }
                },
                move |err| {
                    // 采集失败：迁移任务为 Failed（终态，worker 返回后的 Done 会被拒绝）
                    transition_task_status(
                        &tasks_for_error,
                        &app_for_error,
                        &task_id_for_error,
                        TaskStatus::Failed,
                        Some(format!("监控采集失败: {err}")),
                    );
                },
            );

            // 循环退出时迁移为 Done；若已 Failed 或已被 stop 移除，迁移被拒绝且不发事件
            transition_task_status(&tasks_ref, &app, &task_id, TaskStatus::Done, None);
        });

        Ok(task_info)
    }

    /// 停止指定任务 ID 对应的监控任务
    ///
    /// 设置关闭标志，通知工作线程退出，并从任务 HashMap 中移除句柄。
    ///
    /// # 参数
    /// - `task_id`: 要停止的监控任务 ID
    pub fn stop_monitoring(&self, task_id: &str) {
        let mut tasks = self.tasks.lock().unwrap();
        if let Some(handle) = tasks.remove(task_id) {
            // 通知工作线程退出
            handle.shutdown.store(true, Ordering::Release);
        }
    }

    /// 停止指定会话的全部监控任务；用于后端统一执行 Session teardown。
    pub fn stop_session(&self, session_id: &str) {
        self.tasks.lock().unwrap().retain(|_, handle| {
            let keep = handle.task_info.session_id.as_deref() != Some(session_id);
            if !keep {
                handle.shutdown.store(true, Ordering::Release);
            }
            keep
        });
        self.snapshots.lock().unwrap().remove(session_id);
    }

    /// 获取指定会话的最新监控快照
    ///
    /// # 参数
    /// - `session_id`: 会话 ID
    ///
    /// # 返回
    /// 若存在缓存快照则返回 Some(MonitorSnapshot)，否则返回 None
    pub fn get_monitor_status(&self, session_id: &str) -> Option<MonitorSnapshot> {
        let snapshots = self.snapshots.lock().unwrap();
        snapshots.get(session_id).cloned()
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
    message: Option<String>,
) -> bool {
    let mut tasks = tasks.lock().unwrap();
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
/// - `message`: 可选的附加消息（如错误详情）
fn emit_task_status<R: Runtime>(
    app: &AppHandle<R>,
    task_id: &str,
    status: TaskStatus,
    message: Option<String>,
) {
    let _ = app.emit(
        "task:status",
        TaskStatusEvent {
            task_id: task_id.to_string(),
            status,
            error: message.map(|detail| AppErrorInfo {
                code: "MonitorError".to_string(),
                detail: Some(detail),
            }),
        },
    );
}

#[cfg(test)]
#[path = "monitor_service_test.rs"]
mod service_tests;
