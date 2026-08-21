use crate::core::host_identity::HostKeyVerifier;
use crate::core::process_worker;
use crate::core::shared_exec_registry::SharedExecRegistry;
use crate::errors::app_error::AppErrorInfo;
use crate::errors::app_error::{AppError, ErrorDetail};
use crate::models::host::{AuthType, HostConfig};
use crate::models::monitor::{TaskInfo, TaskStatus};
use crate::models::process::ProcessSnapshot;
use crate::models::session::TaskStatusEvent;
use crate::storage::secure_store;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use tauri::{AppHandle, Emitter, Runtime};
use uuid::Uuid;

/// 进程采样任务句柄，包含任务元数据和关闭信号。
pub(crate) struct ProcessTaskHandle {
    /// 任务基本信息。
    pub(crate) task_info: TaskInfo,
    /// 置为 true 后通知 worker 停止。
    pub(crate) shutdown: Arc<AtomicBool>,
}

/// 进程采样服务：独立管理任务状态与最新全量快照。
#[derive(Clone)]
pub struct ProcessService {
    /// 活跃进程采样任务。
    pub(crate) tasks: Arc<Mutex<HashMap<String, ProcessTaskHandle>>>,
    /// 按 sessionId 缓存最新快照。
    pub(crate) snapshots: Arc<Mutex<HashMap<String, ProcessSnapshot>>>,
    /// 已完成 teardown 的会话，阻止迟到的启动请求重新注册任务。
    closed_sessions: Arc<Mutex<HashSet<String>>>,
    /// 与主机监控共享的采样连接注册表。
    exec_registry: SharedExecRegistry,
}

impl ProcessService {
    /// 创建进程采样服务。
    pub fn new(exec_registry: SharedExecRegistry) -> Self {
        Self {
            tasks: Arc::new(Mutex::new(HashMap::new())),
            snapshots: Arc::new(Mutex::new(HashMap::new())),
            closed_sessions: Arc::new(Mutex::new(HashSet::new())),
            exec_registry,
        }
    }

    /// 为指定会话启动进程采样；凭据读取成功前不注册任务。
    pub fn start_process_monitoring<R: Runtime>(
        &self,
        session_id: String,
        host: HostConfig,
        verifier: HostKeyVerifier,
        app: AppHandle<R>,
    ) -> Result<TaskInfo, AppError> {
        let (password, passphrase) = load_credentials(&host)?;
        let task_id = Uuid::new_v4().to_string();
        let task_info = TaskInfo {
            task_id: task_id.clone(),
            task_type: "process".to_string(),
            session_id: Some(session_id.clone()),
            status: TaskStatus::Pending,
            created_at: chrono::Utc::now().timestamp_millis(),
        };
        let shutdown = Arc::new(AtomicBool::new(false));
        let mut tasks = lock_unpoisoned(&self.tasks);
        if lock_unpoisoned(&self.closed_sessions).contains(&session_id) {
            return Err(AppError::SessionNotFound(session_id.into()));
        }
        tasks.insert(
            task_id.clone(),
            ProcessTaskHandle {
                task_info: task_info.clone(),
                shutdown: shutdown.clone(),
            },
        );
        drop(tasks);

        let tasks_ref = self.tasks.clone();
        let snapshots_ref = self.snapshots.clone();
        let exec_registry = self.exec_registry.clone();
        emit_task_status(&app, &task_id, TaskStatus::Pending, None);

        thread::spawn(move || {
            transition_task_status(&tasks_ref, &app, &task_id, TaskStatus::Running, None);
            let tasks_for_error = tasks_ref.clone();
            let app_for_error = app.clone();
            let task_id_for_error = task_id.clone();
            let tasks_for_snapshot = tasks_ref.clone();
            let app_for_snapshot = app.clone();
            let task_id_for_snapshot = task_id.clone();
            let shutdown_for_snapshot = shutdown.clone();

            run_loop_with_panic_guard(&tasks_ref, &app, &task_id, move || {
                process_worker::run_process_loop(
                    exec_registry,
                    verifier,
                    process_worker::ProcessLoopParams {
                        host,
                        password,
                        passphrase,
                        session_id,
                        shutdown,
                    },
                    move |snapshot| {
                        apply_snapshot_if_task_alive(
                            &tasks_for_snapshot,
                            &snapshots_ref,
                            &app_for_snapshot,
                            &shutdown_for_snapshot,
                            &task_id_for_snapshot,
                            &snapshot,
                        );
                    },
                    move |error| {
                        transition_task_status(
                            &tasks_for_error,
                            &app_for_error,
                            &task_id_for_error,
                            TaskStatus::Failed,
                            process_status_error("进程采集失败: {0}", error.to_string()),
                        );
                    },
                );
            });
        });

        Ok(task_info)
    }

    /// 停止指定进程采样任务并补发 Done 终态事件。
    pub fn stop_process_monitoring<R: Runtime>(&self, app: &AppHandle<R>, task_id: &str) -> bool {
        let Some(handle) = lock_unpoisoned(&self.tasks).remove(task_id) else {
            return false;
        };
        handle.shutdown.store(true, Ordering::Release);
        if !matches!(
            handle.task_info.status,
            TaskStatus::Done | TaskStatus::Failed
        ) {
            emit_task_status(app, task_id, TaskStatus::Done, None);
        }
        true
    }

    /// 停止会话所属的全部进程采样任务并清除快照。
    pub fn stop_session<R: Runtime>(&self, app: &AppHandle<R>, session_id: &str) {
        let mut terminal_ids = Vec::new();
        let mut tasks = lock_unpoisoned(&self.tasks);
        lock_unpoisoned(&self.closed_sessions).insert(session_id.to_string());
        tasks.retain(|task_id, handle| {
            if handle.task_info.session_id.as_deref() != Some(session_id) {
                return true;
            }
            handle.shutdown.store(true, Ordering::Release);
            if !matches!(
                handle.task_info.status,
                TaskStatus::Done | TaskStatus::Failed
            ) {
                terminal_ids.push(task_id.clone());
            }
            false
        });
        drop(tasks);
        lock_unpoisoned(&self.snapshots).remove(session_id);
        for task_id in terminal_ids {
            emit_task_status(app, &task_id, TaskStatus::Done, None);
        }
    }

    /// 停止全部任务并清空缓存。
    pub fn stop_all<R: Runtime>(&self, app: &AppHandle<R>) {
        let task_ids: Vec<_> = lock_unpoisoned(&self.tasks).keys().cloned().collect();
        for task_id in task_ids {
            self.stop_process_monitoring(app, &task_id);
        }
        lock_unpoisoned(&self.snapshots).clear();
    }

    /// 获取会话最新进程快照。
    pub fn get_process_status(&self, session_id: &str) -> Option<ProcessSnapshot> {
        lock_unpoisoned(&self.snapshots).get(session_id).cloned()
    }
}

/// 从安全存储读取运行时凭据，失败时不创建任务。
fn load_credentials(host: &HostConfig) -> Result<(Option<String>, Option<String>), AppError> {
    match host.auth_type {
        AuthType::Password => {
            let password_ref = host.password_ref.as_deref().ok_or_else(|| {
                AppError::InvalidHostConfig(ErrorDetail::msg("密码引用为空", Vec::new()))
            })?;
            Ok((Some(secure_store::get_credential(password_ref)?), None))
        }
        AuthType::PrivateKey => Ok((
            None,
            host.passphrase_ref
                .as_deref()
                .map(secure_store::get_credential)
                .transpose()?,
        )),
    }
}

/// 仅任务仍存活时缓存并推送快照；迟到快照直接丢弃。
pub(crate) fn apply_snapshot_if_task_alive<R: Runtime>(
    tasks: &Arc<Mutex<HashMap<String, ProcessTaskHandle>>>,
    snapshots: &Arc<Mutex<HashMap<String, ProcessSnapshot>>>,
    app: &AppHandle<R>,
    shutdown: &AtomicBool,
    task_id: &str,
    snapshot: &ProcessSnapshot,
) -> bool {
    {
        let tasks_guard = lock_unpoisoned(tasks);
        if !tasks_guard.contains_key(task_id) {
            return false;
        }
        // 与 stop_session 持同一把 tasks 锁，防止清理后迟到快照重新写入缓存。
        lock_unpoisoned(snapshots).insert(snapshot.session_id.clone(), snapshot.clone());
    }
    if let Err(error) = app.emit("process:snapshot", snapshot) {
        handle_snapshot_emit_failure(shutdown, tasks, app, task_id, error);
    }
    true
}

/// 事件推送失败时停止采样并把任务迁移为 Failed。
pub(crate) fn handle_snapshot_emit_failure<R: Runtime>(
    shutdown: &AtomicBool,
    tasks: &Arc<Mutex<HashMap<String, ProcessTaskHandle>>>,
    app: &AppHandle<R>,
    task_id: &str,
    error: impl std::fmt::Display,
) {
    shutdown.store(true, Ordering::Release);
    transition_task_status(
        tasks,
        app,
        task_id,
        TaskStatus::Failed,
        process_status_error("进程快照推送失败: {0}", error.to_string()),
    );
}

/// worker 或回调 panic 时统一迁移 Failed；正常退出迁移 Done。
pub(crate) fn run_loop_with_panic_guard<R: Runtime>(
    tasks: &Arc<Mutex<HashMap<String, ProcessTaskHandle>>>,
    app: &AppHandle<R>,
    task_id: &str,
    body: impl FnOnce(),
) {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(body)) {
        Ok(()) => {
            transition_task_status(tasks, app, task_id, TaskStatus::Done, None);
        }
        Err(payload) => {
            transition_task_status(
                tasks,
                app,
                task_id,
                TaskStatus::Failed,
                process_status_error("进程工作线程异常退出: {0}", panic_message(&*payload)),
            );
        }
    }
}

/// 从 panic payload 提取可诊断文本。
fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|text| (*text).to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".to_string())
}

/// 执行 Pending → Running → Done/Failed 的合法状态迁移。
fn transition_task_status<R: Runtime>(
    tasks: &Arc<Mutex<HashMap<String, ProcessTaskHandle>>>,
    app: &AppHandle<R>,
    task_id: &str,
    status: TaskStatus,
    error: Option<AppErrorInfo>,
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
    emit_task_status(app, task_id, status, error);
    true
}

/// 发布共享任务状态事件。
fn emit_task_status<R: Runtime>(
    app: &AppHandle<R>,
    task_id: &str,
    status: TaskStatus,
    error: Option<AppErrorInfo>,
) {
    let _ = app.emit(
        "task:status",
        TaskStatusEvent {
            task_id: task_id.to_string(),
            status,
            error,
        },
    );
}

/// 构造进程任务错误事件的结构化 payload。
fn process_status_error(key: &str, param: String) -> Option<AppErrorInfo> {
    Some(AppErrorInfo {
        code: "ProcessError".to_string(),
        detail: None,
        detail_key: Some(key.to_string()),
        detail_params: Some(vec![param]),
    })
}

/// 毒化容忍锁，避免单次 worker panic 让服务永久不可用。
fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
#[path = "process_service_test.rs"]
mod tests;
