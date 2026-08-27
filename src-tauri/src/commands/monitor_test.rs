#[cfg(test)]
mod tests {
    use crate::commands::monitor::{get_monitor_status, stop_monitoring};
    use crate::core::monitor_service::MonitorService;
    use crate::core::session_manager::SessionManager;
    use crate::errors::app_error::AppErrorInfo;
    use crate::models::host::{AuthType, HostConfig};
    use crate::models::monitor::{MonitorSnapshot, NetworkSnapshot, TaskStatus};
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;
    use tauri::Manager;
    use tauri::ipc::{CallbackFn, InvokeBody};
    use tauri::test::{INVOKE_KEY, get_ipc_response, mock_builder, mock_context, noop_assets};
    use tauri::webview::InvokeRequest;

    /// 构造同步 IPC 请求；参数走 camelCase 键（与前端 invoke 一致）。
    fn request(cmd: &str, body: serde_json::Value) -> InvokeRequest {
        InvokeRequest {
            cmd: cmd.to_string(),
            callback: CallbackFn(0),
            error: CallbackFn(1),
            url: "http://tauri.localhost".parse().unwrap(),
            body: InvokeBody::Json(body),
            headers: Default::default(),
            invoke_key: INVOKE_KEY.to_string(),
        }
    }

    /// 构造已注册监控命令的 mock 应用；MonitorService 与 SessionManager
    /// 共享同一监控服务实例，返回句柄用于构建 webview。
    fn test_app() -> (tauri::App<tauri::test::MockRuntime>, MonitorService) {
        let session_manager = SessionManager::new();
        let service = session_manager.monitoring();
        let app = mock_builder()
            .manage(session_manager)
            .invoke_handler(tauri::generate_handler![
                stop_monitoring,
                get_monitor_status
            ])
            .build(mock_context(noop_assets()))
            .unwrap();
        (app, service)
    }

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
            group: String::new(),
        }
    }

    /// 构造最小监控快照。
    fn make_snapshot(session_id: &str) -> MonitorSnapshot {
        MonitorSnapshot {
            session_id: session_id.to_string(),
            timestamp: 1_710_000_000_000,
            cpu_usage: Some(1.0),
            memory_usage: Some(2.0),
            memory_total_bytes: Some(6),
            memory_used_bytes: Some(7),
            disk_usage: Some(3.0),
            disk_available_bytes: Some(4),
            disk_total_bytes: Some(5),
            network: NetworkSnapshot {
                available: false,
                interfaces: vec![],
            },
        }
    }

    /// 向注册表插入一个 Running 任务（无工作线程），模拟进行中的监控任务。
    fn insert_running_task(service: &MonitorService, task_id: &str, session_id: &str) {
        service.insert_task_for_test(
            task_id,
            session_id,
            TaskStatus::Running,
            Arc::new(AtomicBool::new(false)),
        );
    }

    /// stop_monitoring 对不存在的 task_id 返回结构化错误 MonitorTaskNotFound：
    /// 前端可区分「任务已停止」与「任务从未存在/早已消失」，暴露陈旧/重复任务状态。
    #[test]
    fn stop_monitoring_unknown_task_returns_typed_error() {
        let (app, _service) = test_app();
        let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .unwrap();

        let response = get_ipc_response(
            &webview,
            request(
                "stop_monitoring",
                serde_json::json!({ "taskId": "task-never-existed" }),
            ),
        )
        .expect_err("未知任务必须返回错误");

        let error: AppErrorInfo = serde_json::from_value(response).expect("错误应可解析");
        assert_eq!(error.code, "MonitorTaskNotFound");
    }

    /// stop_monitoring 对注册表中的任务返回 Ok，并移除句柄。
    #[test]
    fn stop_monitoring_existing_task_returns_ok() {
        let (app, service) = test_app();
        insert_running_task(&service, "task-1", "session-1");
        let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .unwrap();

        let response = get_ipc_response(
            &webview,
            request("stop_monitoring", serde_json::json!({ "taskId": "task-1" })),
        )
        .expect("注册表中的任务 stop 应成功");

        let value: serde_json::Value = response.deserialize().unwrap();
        assert_eq!(value, serde_json::Value::Null);
        assert!(!service.task_exists_for_test("task-1"));
    }

    /// 回归：会话存活但尚无快照（首轮采集前、或监控已停止）时，get_monitor_status
    /// 必须返回 MonitorSnapshotUnavailable 而非 SessionNotFound；SessionNotFound
    /// 是 close_session 式 teardown 的键，瞬时无数据不得触发前端拆除会话状态。
    #[test]
    fn get_monitor_status_live_session_without_snapshot_returns_unavailable() {
        let (app, _service) = test_app();
        app.state::<SessionManager>()
            .insert_session_for_test("session-1", make_host());
        let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .unwrap();

        let response = get_ipc_response(
            &webview,
            request(
                "get_monitor_status",
                serde_json::json!({ "sessionId": "session-1" }),
            ),
        )
        .expect_err("尚无快照必须返回错误");

        let error: AppErrorInfo = serde_json::from_value(response).expect("错误应可解析");
        assert_eq!(error.code, "MonitorSnapshotUnavailable");
    }

    /// 会话确实不存在时仍返回 SessionNotFound：保留 teardown 键的语义。
    #[test]
    fn get_monitor_status_missing_session_returns_session_not_found() {
        let (app, _service) = test_app();
        let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .unwrap();

        let response = get_ipc_response(
            &webview,
            request(
                "get_monitor_status",
                serde_json::json!({ "sessionId": "ghost-session" }),
            ),
        )
        .expect_err("会话不存在必须返回错误");

        let error: AppErrorInfo = serde_json::from_value(response).expect("错误应可解析");
        assert_eq!(error.code, "SessionNotFound");
    }

    /// 会话存活且已有快照时正常返回快照。
    #[test]
    fn get_monitor_status_existing_snapshot_returns_ok() {
        let (app, service) = test_app();
        app.state::<SessionManager>()
            .insert_session_for_test("session-1", make_host());
        service.insert_snapshot_for_test(make_snapshot("session-1"));
        let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .unwrap();

        let response = get_ipc_response(
            &webview,
            request(
                "get_monitor_status",
                serde_json::json!({ "sessionId": "session-1" }),
            ),
        )
        .expect("已有快照时查询应成功");

        let snapshot: MonitorSnapshot = response.deserialize().expect("响应应为监控快照");
        assert_eq!(snapshot.session_id, "session-1");
    }
}
