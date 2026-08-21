#[cfg(test)]
mod tests {
    use crate::commands::process::{
        get_process_status, start_process_monitoring, stop_process_monitoring,
    };
    use crate::core::host_identity::HostIdentityService;
    use crate::core::monitor_service::MonitorService;
    use crate::core::process_service::ProcessService;
    use crate::core::session_manager::SessionManager;
    use crate::core::sftp_service::SftpService;
    use crate::core::shared_exec_registry::SharedExecRegistry;
    use crate::errors::app_error::AppErrorInfo;
    use crate::models::host::{AuthType, HostConfig};
    use crate::models::process::{ProcessInfo, ProcessSnapshot};
    use tauri::Manager;
    use tauri::ipc::{CallbackFn, InvokeBody};
    use tauri::test::{INVOKE_KEY, get_ipc_response, mock_builder, mock_context, noop_assets};
    use tauri::webview::InvokeRequest;

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

    fn host() -> HostConfig {
        HostConfig {
            id: "host-1".to_string(),
            name: "test".to_string(),
            host: "127.0.0.1".to_string(),
            port: 22,
            username: "root".to_string(),
            auth_type: AuthType::PrivateKey,
            password_ref: None,
            private_key_path: Some("/tmp/test-key".to_string()),
            passphrase_ref: None,
            remark: None,
            group: String::new(),
        }
    }

    fn test_app() -> (tauri::App<tauri::test::MockRuntime>, ProcessService) {
        let registry = SharedExecRegistry::new();
        let process_service = ProcessService::new(registry.clone());
        let manager = SessionManager::new(
            MonitorService::new(registry.clone()),
            process_service.clone(),
            SftpService::new(),
            HostIdentityService::new(),
            registry,
        );
        let app = mock_builder()
            .manage(process_service.clone())
            .manage(manager)
            .invoke_handler(tauri::generate_handler![
                start_process_monitoring,
                stop_process_monitoring,
                get_process_status
            ])
            .build(mock_context(noop_assets()))
            .unwrap();
        (app, process_service)
    }

    #[test]
    fn start_for_missing_session_returns_session_not_found() {
        let (app, _) = test_app();
        let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .unwrap();
        let response = get_ipc_response(
            &webview,
            request(
                "start_process_monitoring",
                serde_json::json!({"sessionId": "ghost"}),
            ),
        )
        .expect_err("missing session should fail start");
        let error: AppErrorInfo = serde_json::from_value(response).unwrap();
        assert_eq!(error.code, "SessionNotFound");
    }

    #[test]
    fn stop_unknown_process_task_returns_typed_error() {
        let (app, _) = test_app();
        let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .unwrap();
        let response = get_ipc_response(
            &webview,
            request(
                "stop_process_monitoring",
                serde_json::json!({"taskId": "missing"}),
            ),
        )
        .expect_err("unknown process task should fail");
        let error: AppErrorInfo = serde_json::from_value(response).unwrap();
        assert_eq!(error.code, "ProcessTaskNotFound");
    }

    #[test]
    fn process_status_distinguishes_missing_session_and_missing_snapshot() {
        let (app, _) = test_app();
        let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .unwrap();
        let missing_session = get_ipc_response(
            &webview,
            request(
                "get_process_status",
                serde_json::json!({"sessionId": "ghost"}),
            ),
        )
        .expect_err("missing session should fail");
        let missing_session: AppErrorInfo = serde_json::from_value(missing_session).unwrap();
        assert_eq!(missing_session.code, "SessionNotFound");

        app.state::<SessionManager>()
            .insert_session_for_test("session-1", host());
        let missing_snapshot = get_ipc_response(
            &webview,
            request(
                "get_process_status",
                serde_json::json!({"sessionId": "session-1"}),
            ),
        )
        .expect_err("missing snapshot should fail");
        let missing_snapshot: AppErrorInfo = serde_json::from_value(missing_snapshot).unwrap();
        assert_eq!(missing_snapshot.code, "ProcessSnapshotUnavailable");
    }

    #[test]
    fn process_status_returns_cached_snapshot() {
        let (app, service) = test_app();
        app.state::<SessionManager>()
            .insert_session_for_test("session-1", host());
        service.snapshots.lock().unwrap().insert(
            "session-1".to_string(),
            ProcessSnapshot {
                session_id: "session-1".to_string(),
                timestamp: 1_710_000_000_000,
                processes: vec![ProcessInfo {
                    pid: 1,
                    ppid: 0,
                    user: "root".to_string(),
                    command: "init".to_string(),
                    command_line: "init".to_string(),
                    cpu_percent: None,
                    memory_bytes: Some(1),
                    state: "S".to_string(),
                }],
                total_count: 1,
            },
        );
        let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .unwrap();
        let response = get_ipc_response(
            &webview,
            request(
                "get_process_status",
                serde_json::json!({"sessionId": "session-1"}),
            ),
        )
        .expect("cached snapshot should be returned");
        let snapshot: ProcessSnapshot = response.deserialize().unwrap();
        assert_eq!(snapshot.total_count, 1);
    }
}
