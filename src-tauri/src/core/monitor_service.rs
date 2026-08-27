use crate::core::host_identity::HostKeyVerifier;
use crate::core::monitor_worker;
use crate::core::sampling_task_runtime::{
    SamplingTaskRuntime, SamplingTaskSink, SamplingTaskSpec, SamplingWorkerInput,
};
use crate::core::shared_exec_registry::SharedExecRegistry;
use crate::errors::app_error::AppError;
use crate::models::host::HostConfig;
#[cfg(test)]
use crate::models::monitor::TaskStatus;
use crate::models::monitor::{MonitorSnapshot, TaskInfo};
#[cfg(test)]
use std::sync::Arc;
use tauri::{AppHandle, Runtime};

/// 监控任务的固定事件与错误描述。
const SPEC: SamplingTaskSpec = SamplingTaskSpec {
    task_type: "monitor",
    snapshot_event: "monitor:snapshot",
    error_code: "MonitorError",
    worker_panic_detail_key: "监控工作线程异常退出: {0}",
    snapshot_emit_detail_key: "监控快照推送失败: {0}",
};

/// 监控 adapter：只连接监控 worker 与共享任务生命周期。
#[derive(Clone)]
pub struct MonitorService {
    runtime: SamplingTaskRuntime<MonitorSnapshot>,
    exec_registry: SharedExecRegistry,
}

impl MonitorService {
    /// 创建监控 adapter，并绑定 Session teardown 共用的 exec 注册表。
    pub fn new(exec_registry: SharedExecRegistry) -> Self {
        Self {
            runtime: SamplingTaskRuntime::new(SPEC),
            exec_registry,
        }
    }

    /// 读取凭据并启动真实监控采样 worker。
    pub fn start_monitoring<R: Runtime>(
        &self,
        session_id: String,
        host: HostConfig,
        verifier: HostKeyVerifier,
        app: AppHandle<R>,
    ) -> Result<TaskInfo, AppError> {
        let exec_registry = self.exec_registry.clone();
        self.runtime.start(
            app,
            session_id,
            host,
            move |input: SamplingWorkerInput, sink: SamplingTaskSink<R, MonitorSnapshot>| {
                let snapshot_sink = sink.clone();
                monitor_worker::run_monitor_loop(
                    exec_registry,
                    verifier,
                    monitor_worker::MonitorLoopParams {
                        host: input.host,
                        password: input.password,
                        passphrase: input.passphrase,
                        session_id: input.session_id,
                        shutdown: input.shutdown,
                    },
                    move |snapshot| snapshot_sink.publish(snapshot),
                    move |error| sink.fail("监控采集失败: {0}", error.to_string()),
                );
            },
        )
    }

    /// 停止一个监控任务并在需要时补发 Done。
    pub fn stop_monitoring<R: Runtime>(&self, app: &AppHandle<R>, task_id: &str) -> bool {
        self.runtime.stop(app, task_id)
    }

    /// 建立 Session tombstone 并停止该 Session 的全部监控任务。
    pub fn stop_session<R: Runtime>(&self, app: &AppHandle<R>, session_id: &str) {
        self.runtime.stop_session(app, session_id);
    }

    /// 停止全部监控任务并清空快照。
    pub fn stop_all<R: Runtime>(&self, app: &AppHandle<R>) {
        self.runtime.stop_all(app);
    }

    /// 返回指定 Session 最近一次成功采集的监控快照。
    pub fn get_monitor_status(&self, session_id: &str) -> Option<MonitorSnapshot> {
        self.runtime.latest_snapshot(session_id)
    }

    /// 测试构造：注入缓存快照供命令层有数据路径测试使用。
    #[cfg(test)]
    pub(crate) fn insert_snapshot_for_test(&self, snapshot: MonitorSnapshot) {
        let session_id = snapshot.session_id.clone();
        self.runtime.insert_snapshot_for_test(&session_id, snapshot);
    }

    /// 测试构造：注入任务供命令与 Session teardown 测试使用。
    #[cfg(test)]
    pub(crate) fn insert_task_for_test(
        &self,
        task_id: &str,
        session_id: &str,
        status: TaskStatus,
        shutdown: Arc<std::sync::atomic::AtomicBool>,
    ) {
        self.runtime
            .insert_task_for_test(task_id, session_id, status, shutdown);
    }

    /// 测试观测：确认任务已从 runtime registry 移除。
    #[cfg(test)]
    pub(crate) fn task_exists_for_test(&self, task_id: &str) -> bool {
        self.runtime.task_exists_for_test(task_id)
    }
}

#[cfg(test)]
#[path = "monitor_service_test.rs"]
mod service_tests;
