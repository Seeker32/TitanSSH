use crate::errors::app_error::{AppError, AppErrorInfo, ErrorDetail};
use crate::models::host::{AuthType, HostConfig};
use crate::models::monitor::{TaskInfo, TaskStatus};
use crate::models::session::TaskStatusEvent;
use crate::storage::secure_store;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use tauri::{AppHandle, Emitter, Runtime};
use uuid::Uuid;

/// 采样任务 adapter 的固定事件与错误描述。
#[derive(Clone, Copy)]
pub(crate) struct SamplingTaskSpec {
    pub(crate) task_type: &'static str,
    pub(crate) snapshot_event: &'static str,
    pub(crate) error_code: &'static str,
    pub(crate) worker_panic_detail_key: &'static str,
    pub(crate) snapshot_emit_detail_key: &'static str,
}

/// 领域 worker 的统一输入；凭据只在 worker 生命周期内保存在内存中。
pub(crate) struct SamplingWorkerInput {
    pub(crate) host: HostConfig,
    pub(crate) password: Option<String>,
    pub(crate) passphrase: Option<String>,
    pub(crate) session_id: String,
    pub(crate) shutdown: Arc<AtomicBool>,
}

/// 任务注册表中的内部句柄。
struct SamplingTaskHandle {
    task_info: TaskInfo,
    shutdown: Arc<AtomicBool>,
}

/// 任务、快照与 Session tombstone 的单一权威状态。
struct SamplingTaskState<S> {
    tasks: HashMap<String, SamplingTaskHandle>,
    snapshots: HashMap<String, S>,
    closed_sessions: HashSet<String>,
}

/// 领域 worker 使用的生命周期与快照发布接口。
pub(crate) struct SamplingTaskSink<R: Runtime, S> {
    app: AppHandle<R>,
    task_id: String,
    state: Arc<Mutex<SamplingTaskState<S>>>,
    spec: SamplingTaskSpec,
}

impl<R: Runtime, S> Clone for SamplingTaskSink<R, S> {
    /// 复制发送句柄；底层任务状态保持共享。
    fn clone(&self) -> Self {
        Self {
            app: self.app.clone(),
            task_id: self.task_id.clone(),
            state: self.state.clone(),
            spec: self.spec,
        }
    }
}

impl<R: Runtime, S> SamplingTaskSink<R, S>
where
    S: Clone + Serialize + Send + 'static,
{
    /// 仅在任务仍为 Running 时缓存并发送一份快照；事件发送失败会终止任务。
    pub(crate) fn publish(&self, snapshot: S) {
        let mut state = lock_unpoisoned(&self.state);
        let Some(session_id) = state.tasks.get(&self.task_id).and_then(|handle| {
            (handle.task_info.status == TaskStatus::Running)
                .then(|| handle.task_info.session_id.clone())
                .flatten()
        }) else {
            return;
        };

        state.snapshots.insert(session_id.clone(), snapshot.clone());
        if let Err(error) = self.app.emit(self.spec.snapshot_event, &snapshot) {
            let detail = error.to_string();
            if let Some(handle) = state.tasks.get_mut(&self.task_id) {
                handle.shutdown.store(true, Ordering::Release);
                if handle.task_info.status == TaskStatus::Running {
                    handle.task_info.status = TaskStatus::Failed;
                    emit_task_status(
                        &self.app,
                        &self.task_id,
                        self.spec.task_type,
                        Some(&session_id),
                        TaskStatus::Failed,
                        status_error(
                            self.spec.error_code,
                            self.spec.snapshot_emit_detail_key,
                            detail,
                        ),
                    );
                }
            }
        }
    }

    /// 仅把仍在 Running 的任务迁移为 Failed。
    pub(crate) fn fail(&self, detail_key: &'static str, detail_param: String) {
        let mut state = lock_unpoisoned(&self.state);
        let Some(handle) = state.tasks.get_mut(&self.task_id) else {
            return;
        };
        if handle.task_info.status != TaskStatus::Running {
            return;
        }
        handle.task_info.status = TaskStatus::Failed;
        emit_task_status(
            &self.app,
            &self.task_id,
            self.spec.task_type,
            handle.task_info.session_id.as_deref(),
            TaskStatus::Failed,
            status_error(self.spec.error_code, detail_key, detail_param),
        );
    }
}

/// 采样任务的共享生命周期 runtime；不承载任何领域采样逻辑。
#[derive(Clone)]
pub(crate) struct SamplingTaskRuntime<S> {
    spec: SamplingTaskSpec,
    state: Arc<Mutex<SamplingTaskState<S>>>,
}

impl<S> SamplingTaskRuntime<S>
where
    S: Clone + Serialize + Send + 'static,
{
    /// 使用 adapter 的固定描述创建空 runtime。
    pub(crate) fn new(spec: SamplingTaskSpec) -> Self {
        Self {
            spec,
            state: Arc::new(Mutex::new(SamplingTaskState {
                tasks: HashMap::new(),
                snapshots: HashMap::new(),
                closed_sessions: HashSet::new(),
            })),
        }
    }

    /// 读取凭据、原子注册 Pending 任务并启动受保护的领域 worker。
    pub(crate) fn start<R, F>(
        &self,
        app: AppHandle<R>,
        session_id: String,
        host: HostConfig,
        worker: F,
    ) -> Result<TaskInfo, AppError>
    where
        R: Runtime,
        F: FnOnce(SamplingWorkerInput, SamplingTaskSink<R, S>) + Send + 'static,
    {
        let (password, passphrase) = load_credentials(&host)?;
        let task_id = Uuid::new_v4().to_string();
        let task_info = TaskInfo {
            task_id: task_id.clone(),
            task_type: self.spec.task_type.to_string(),
            session_id: Some(session_id.clone()),
            status: TaskStatus::Pending,
            created_at: chrono::Utc::now().timestamp_millis(),
        };
        let shutdown = Arc::new(AtomicBool::new(false));
        let mut state = lock_unpoisoned(&self.state);
        if state.closed_sessions.contains(&session_id) {
            return Err(AppError::SessionNotFound(session_id.into()));
        }
        state.tasks.insert(
            task_id.clone(),
            SamplingTaskHandle {
                task_info: task_info.clone(),
                shutdown: shutdown.clone(),
            },
        );
        emit_task_status(
            &app,
            &task_id,
            self.spec.task_type,
            Some(&session_id),
            TaskStatus::Pending,
            None,
        );
        drop(state);

        let runtime = self.clone();
        thread::spawn(move || {
            // teardown 已移除任务时不允许领域 worker 建立连接或采样。
            if !runtime.transition(&app, &task_id, TaskStatus::Running, None) {
                return;
            }
            let input = SamplingWorkerInput {
                host,
                password,
                passphrase,
                session_id,
                shutdown,
            };
            let sink = SamplingTaskSink {
                app: app.clone(),
                task_id: task_id.clone(),
                state: runtime.state.clone(),
                spec: runtime.spec,
            };
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| worker(input, sink))) {
                Ok(()) => {
                    runtime.transition(&app, &task_id, TaskStatus::Done, None);
                }
                Err(payload) => {
                    runtime.transition(
                        &app,
                        &task_id,
                        TaskStatus::Failed,
                        status_error(
                            runtime.spec.error_code,
                            runtime.spec.worker_panic_detail_key,
                            panic_message(&*payload),
                        ),
                    );
                }
            }
        });

        Ok(task_info)
    }

    /// 将任务迁移到合法状态；状态更新与事件发送持同一把锁。
    fn transition<R: Runtime>(
        &self,
        app: &AppHandle<R>,
        task_id: &str,
        status: TaskStatus,
        error: Option<AppErrorInfo>,
    ) -> bool {
        let mut state = lock_unpoisoned(&self.state);
        let Some(handle) = state.tasks.get_mut(task_id) else {
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
        emit_task_status(
            app,
            task_id,
            self.spec.task_type,
            handle.task_info.session_id.as_deref(),
            status,
            error,
        );
        true
    }

    /// 原子移除任务并通知其 worker 停止；不存在时返回 false。
    pub(crate) fn stop<R: Runtime>(&self, app: &AppHandle<R>, task_id: &str) -> bool {
        let handle = {
            let mut state = lock_unpoisoned(&self.state);
            let Some(handle) = state.tasks.remove(task_id) else {
                return false;
            };
            handle.shutdown.store(true, Ordering::Release);
            handle
        };
        if !matches!(
            handle.task_info.status,
            TaskStatus::Done | TaskStatus::Failed
        ) {
            emit_task_status(
                app,
                task_id,
                self.spec.task_type,
                handle.task_info.session_id.as_deref(),
                TaskStatus::Done,
                None,
            );
        }
        true
    }

    /// 建立 Session tombstone，并原子清理该 Session 的任务与快照。
    pub(crate) fn stop_session<R: Runtime>(&self, app: &AppHandle<R>, session_id: &str) {
        let terminal_ids = {
            let mut state = lock_unpoisoned(&self.state);
            state.closed_sessions.insert(session_id.to_string());
            let mut terminal_ids = Vec::new();
            state.tasks.retain(|task_id, handle| {
                if handle.task_info.session_id.as_deref() != Some(session_id) {
                    return true;
                }
                handle.shutdown.store(true, Ordering::Release);
                if !matches!(
                    handle.task_info.status,
                    TaskStatus::Done | TaskStatus::Failed
                ) {
                    terminal_ids.push((task_id.clone(), handle.task_info.session_id.clone()));
                }
                false
            });
            state.snapshots.remove(session_id);
            terminal_ids
        };
        for (task_id, session_id) in terminal_ids {
            emit_task_status(
                app,
                &task_id,
                self.spec.task_type,
                session_id.as_deref(),
                TaskStatus::Done,
                None,
            );
        }
    }

    /// 原子停止全部任务并清空快照；应用退出时使用。
    pub(crate) fn stop_all<R: Runtime>(&self, app: &AppHandle<R>) {
        let terminal_ids = {
            let mut state = lock_unpoisoned(&self.state);
            let mut terminal_ids = Vec::new();
            for (task_id, handle) in state.tasks.drain() {
                handle.shutdown.store(true, Ordering::Release);
                if !matches!(
                    handle.task_info.status,
                    TaskStatus::Done | TaskStatus::Failed
                ) {
                    terminal_ids.push((task_id, handle.task_info.session_id.clone()));
                }
            }
            state.snapshots.clear();
            terminal_ids
        };
        for (task_id, session_id) in terminal_ids {
            emit_task_status(
                app,
                &task_id,
                self.spec.task_type,
                session_id.as_deref(),
                TaskStatus::Done,
                None,
            );
        }
    }

    /// 返回指定 Session 最近一次成功提交的快照。
    pub(crate) fn latest_snapshot(&self, session_id: &str) -> Option<S> {
        lock_unpoisoned(&self.state)
            .snapshots
            .get(session_id)
            .cloned()
    }

    #[cfg(test)]
    /// 测试构造：注入指定 Session 的缓存快照。
    pub(crate) fn insert_snapshot_for_test(&self, session_id: &str, snapshot: S) {
        lock_unpoisoned(&self.state)
            .snapshots
            .insert(session_id.to_string(), snapshot);
    }

    #[cfg(test)]
    /// 测试构造：注入任务句柄，避免测试依赖领域专属任务类型。
    pub(crate) fn insert_task_for_test(
        &self,
        task_id: &str,
        session_id: &str,
        status: TaskStatus,
        shutdown: Arc<AtomicBool>,
    ) {
        lock_unpoisoned(&self.state).tasks.insert(
            task_id.to_string(),
            SamplingTaskHandle {
                task_info: TaskInfo {
                    task_id: task_id.to_string(),
                    task_type: self.spec.task_type.to_string(),
                    session_id: Some(session_id.to_string()),
                    status,
                    created_at: 0,
                },
                shutdown,
            },
        );
    }

    #[cfg(test)]
    /// 测试观测：确认任务是否仍在 runtime registry 中。
    pub(crate) fn task_exists_for_test(&self, task_id: &str) -> bool {
        lock_unpoisoned(&self.state).tasks.contains_key(task_id)
    }
}

/// 从安全存储读取采样 worker 所需凭据；失败时 runtime 不发生变化。
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

/// 构造稳定的采样任务结构化错误。
fn status_error(code: &str, key: &str, param: String) -> Option<AppErrorInfo> {
    Some(AppErrorInfo {
        code: code.to_string(),
        detail: None,
        detail_key: Some(key.to_string()),
        detail_params: Some(vec![param]),
    })
}

/// 发布共享任务状态事件；发送失败不改变已更新的 registry 状态。
fn emit_task_status<R: Runtime>(
    app: &AppHandle<R>,
    task_id: &str,
    task_type: &str,
    session_id: Option<&str>,
    status: TaskStatus,
    error: Option<AppErrorInfo>,
) {
    let Some(session_id) = session_id else {
        return;
    };
    let _ = app.emit(
        "task:status",
        TaskStatusEvent {
            task_id: task_id.to_string(),
            task_type: task_type.to_string(),
            session_id: session_id.to_string(),
            status,
            error,
        },
    );
}

/// 提取 worker panic payload 的可诊断文本。
fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|text| (*text).to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".to_string())
}

/// 毒化容忍锁；一次 worker panic 不应令后续 Session 永久不可用。
fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
#[path = "sampling_task_runtime_test.rs"]
mod tests;
