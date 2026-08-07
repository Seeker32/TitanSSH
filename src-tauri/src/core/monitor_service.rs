use crate::core::monitor_worker;
use crate::errors::app_error::AppError;
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
            // 更新任务状态为 Running
            update_task_status(&tasks_ref, &task_id, TaskStatus::Running);
            emit_task_status(&app, &task_id, TaskStatus::Running, None);

            let tasks_for_error = Arc::clone(&tasks_ref);
            let app_for_error = app.clone();
            let task_id_for_error = task_id.clone();

            let tasks_for_snap = Arc::clone(&tasks_ref);
            let app_for_snap = app.clone();
            let task_id_for_snap = task_id.clone();
            let session_id_for_snap = session_id.clone();

            monitor_worker::run_monitor_loop(
                host,
                password,
                passphrase,
                session_id,
                shutdown,
                move |snapshot| {
                    // 更新快照缓存
                    {
                        let mut snapshots = snapshots_ref.lock().unwrap();
                        snapshots.insert(session_id_for_snap.clone(), snapshot.clone());
                    }
                    // 推送事件到前端，失败则标记任务为 Failed
                    if let Err(err) = app_for_snap.emit("monitor:snapshot", &snapshot) {
                        update_task_status(&tasks_for_snap, &task_id_for_snap, TaskStatus::Failed);
                        emit_task_status(
                            &app_for_snap,
                            &task_id_for_snap,
                            TaskStatus::Failed,
                            Some(format!("监控快照推送失败: {err}")),
                        );
                    }
                },
                move |err| {
                    // 采集失败：更新任务状态为 Failed 并派发事件
                    update_task_status(&tasks_for_error, &task_id_for_error, TaskStatus::Failed);
                    emit_task_status(
                        &app_for_error,
                        &task_id_for_error,
                        TaskStatus::Failed,
                        Some(format!("监控采集失败: {err}")),
                    );
                },
            );

            // run_monitor_loop 正常退出（shutdown=true）时更新为 Done
            update_task_status(&tasks_ref, &task_id, TaskStatus::Done);
            emit_task_status(&app, &task_id, TaskStatus::Done, None);
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

/// 更新任务 HashMap 中指定任务的状态
///
/// # 参数
/// - `tasks`: 任务 HashMap 的共享引用
/// - `task_id`: 要更新的任务 ID
/// - `status`: 新的任务状态
fn update_task_status(
    tasks: &Arc<Mutex<HashMap<String, MonitorTaskHandle>>>,
    task_id: &str,
    status: TaskStatus,
) {
    let mut tasks = tasks.lock().unwrap();
    if let Some(handle) = tasks.get_mut(task_id) {
        handle.task_info.status = status;
    }
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
            message,
        },
    );
}

#[cfg(test)]
mod service_tests {
    use super::*;
    use crate::models::host::{AuthType, HostConfig};

    /// 构造测试用 HostConfig
    fn make_host() -> HostConfig {
        HostConfig {
            id: "h1".to_string(),
            name: "test".to_string(),
            host: "127.0.0.1".to_string(),
            port: 22,
            username: "root".to_string(),
            auth_type: AuthType::PrivateKey,
            password_ref: None,
            private_key_path: Some("/tmp/test-key".to_string()),
            passphrase_ref: None,
            remark: None,
        }
    }

    /// 缺少密码引用时必须原子失败，不创建监控任务。
    #[test]
    fn start_monitoring_rejects_missing_password_before_task_creation() {
        use std::sync::atomic::AtomicUsize;
        use tauri::Listener;
        use tauri::test::mock_app;

        let app = mock_app();
        let service = MonitorService::new();
        let mut host = make_host();
        host.auth_type = AuthType::Password;
        host.password_ref = None;
        let emitted_events = Arc::new(AtomicUsize::new(0));
        let event_counter = emitted_events.clone();
        app.listen("task:status", move |_| {
            event_counter.fetch_add(1, Ordering::Relaxed);
        });

        let result = service.start_monitoring("session-1".to_string(), host, app.handle().clone());

        assert!(matches!(result, Err(AppError::InvalidHostConfig(_))));
        assert!(service.tasks.lock().unwrap().is_empty());
        assert_eq!(emitted_events.load(Ordering::Relaxed), 0);
    }

    /// start_monitoring 返回的 TaskInfo 初始状态为 Pending，task_id 非空
    #[test]
    fn start_monitoring_initial_task_is_pending() {
        use tauri::test::mock_app;
        let app = mock_app();
        let service = MonitorService::new();
        let task = service
            .start_monitoring("session-1".to_string(), make_host(), app.handle().clone())
            .unwrap();
        assert_eq!(task.status, TaskStatus::Pending);
        assert!(!task.task_id.is_empty());
        assert_eq!(task.session_id, Some("session-1".to_string()));
    }

    /// stop_monitoring 设置关闭标志后任务从 HashMap 中移除
    #[test]
    fn stop_monitoring_removes_task_handle() {
        use tauri::test::mock_app;
        let app = mock_app();
        let service = MonitorService::new();
        let task = service
            .start_monitoring("session-1".to_string(), make_host(), app.handle().clone())
            .unwrap();
        service.stop_monitoring(&task.task_id);
        // 任务已从 HashMap 移除
        let tasks = service.tasks.lock().unwrap();
        assert!(!tasks.contains_key(&task.task_id));
    }

    /// stop_session 只清理目标 Session 的任务与快照，不影响其他 Session。
    #[test]
    fn stop_session_cleans_only_matching_monitor_state() {
        let service = MonitorService::new();
        let target_shutdown = Arc::new(AtomicBool::new(false));
        let other_shutdown = Arc::new(AtomicBool::new(false));

        for (task_id, session_id, shutdown) in [
            ("target-task", "target-session", target_shutdown.clone()),
            ("other-task", "other-session", other_shutdown.clone()),
        ] {
            service.tasks.lock().unwrap().insert(
                task_id.to_string(),
                MonitorTaskHandle {
                    task_info: TaskInfo {
                        task_id: task_id.to_string(),
                        task_type: "monitor".to_string(),
                        session_id: Some(session_id.to_string()),
                        status: TaskStatus::Running,
                        created_at: 1_710_000_000_000,
                    },
                    shutdown,
                },
            );
            service.snapshots.lock().unwrap().insert(
                session_id.to_string(),
                MonitorSnapshot {
                    session_id: session_id.to_string(),
                    timestamp: 1_710_000_000_000,
                    cpu_usage: 10.0,
                    memory_usage: 20.0,
                    disk_usage: 30.0,
                    disk_available_bytes: 40,
                    disk_total_bytes: 50,
                },
            );
        }

        service.stop_session("target-session");

        assert!(target_shutdown.load(Ordering::Acquire));
        assert!(!other_shutdown.load(Ordering::Acquire));
        assert!(service.get_monitor_status("target-session").is_none());
        assert!(service.get_monitor_status("other-session").is_some());
        assert!(service.tasks.lock().unwrap().contains_key("other-task"));
    }
}
