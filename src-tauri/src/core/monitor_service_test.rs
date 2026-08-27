#[cfg(test)]
mod service_tests {
    use crate::core::host_identity::HostKeyVerifier;
    use crate::core::monitor_service::MonitorService;
    use crate::core::shared_exec_registry::{ExecConnectionEntry, SharedExecRegistry};
    use crate::core::ssh_transport::ExecTransport;
    use crate::core::ssh_transport::test_support::repeating_exec;
    use crate::errors::app_error::AppError;
    use crate::models::host::{AuthType, HostConfig};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::thread;
    use std::time::{Duration, Instant};
    use tauri::Listener;
    use tauri::test::mock_app;

    struct FakeEntry(&'static str);

    impl ExecConnectionEntry for FakeEntry {
        /// 返回固定输出的 mock exec capability。
        fn exec_transport(&self) -> ExecTransport {
            repeating_exec(self.0.to_string())
        }
    }

    fn verifier() -> HostKeyVerifier {
        Arc::new(|_| Ok(()))
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

    fn wait_until(mut condition: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while !condition() {
            assert!(Instant::now() < deadline, "监控测试等待超时");
            thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn shared_exec_worker_produces_monitor_snapshot() {
        let app = mock_app();
        let registry = SharedExecRegistry::new();
        registry.insert(
            "session-1",
            FakeEntry(
                "CPU_TOTAL=100\nCPU_IDLE=20\nMEM_TOTAL_KB=1000\nMEM_AVAILABLE_KB=500\nDISK=25\nDISK_AVAIL=750\nDISK_TOTAL=1000",
            ),
        );
        let service = MonitorService::new(registry);
        let task = service
            .start_monitoring(
                "session-1".to_string(),
                host(),
                verifier(),
                app.handle().clone(),
            )
            .unwrap();

        wait_until(|| service.get_monitor_status("session-1").is_some());
        assert_eq!(
            service.get_monitor_status("session-1").unwrap().disk_usage,
            Some(25.0)
        );
        assert!(service.stop_monitoring(app.handle(), &task.task_id));
    }

    #[test]
    fn monitor_worker_error_maps_to_monitor_error() {
        let app = mock_app();
        let registry = SharedExecRegistry::new();
        registry.insert("session-1", FakeEntry("BROKEN"));
        let service = MonitorService::new(registry);
        let failed = Arc::new(AtomicUsize::new(0));
        let failed_ref = failed.clone();
        app.listen("task:status", move |event| {
            let payload: serde_json::Value = serde_json::from_str(event.payload()).unwrap();
            if payload["status"] == "Failed" {
                assert_eq!(payload["taskType"], "monitor");
                assert_eq!(payload["sessionId"], "session-1");
                assert_eq!(payload["error"]["code"], "MonitorError");
                failed_ref.fetch_add(1, Ordering::Release);
            }
        });

        service
            .start_monitoring(
                "session-1".to_string(),
                host(),
                verifier(),
                app.handle().clone(),
            )
            .unwrap();
        wait_until(|| failed.load(Ordering::Acquire) == 1);
    }

    #[test]
    fn missing_password_reference_does_not_register_task() {
        let app = mock_app();
        let service = MonitorService::new(SharedExecRegistry::new());
        let mut invalid_host = host();
        invalid_host.auth_type = AuthType::Password;
        invalid_host.password_ref = None;

        let result = service.start_monitoring(
            "session-1".to_string(),
            invalid_host,
            verifier(),
            app.handle().clone(),
        );

        assert!(matches!(result, Err(AppError::InvalidHostConfig(_))));
        assert!(!service.task_exists_for_test("any-task"));
    }
}
