#[cfg(test)]
mod tests {
    use crate::core::host_identity::HostKeyVerifier;
    use crate::core::process_service::ProcessService;
    use crate::core::shared_exec_registry::{ExecConnectionEntry, SharedExecRegistry};
    use crate::core::ssh_transport::ExecTransport;
    use crate::core::ssh_transport::test_support::repeating_exec;
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

    fn verifier() -> HostKeyVerifier {
        Arc::new(|_| Ok(()))
    }

    fn wait_until(mut condition: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while !condition() {
            assert!(Instant::now() < deadline, "进程测试等待超时");
            thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn shared_exec_worker_produces_process_snapshot() {
        let app = mock_app();
        let registry = SharedExecRegistry::new();
        registry.insert(
            "session-1",
            FakeEntry(
                "PLATFORM=linux\nHZ=100\nP\t1\t0\tR\t10\t5\t4096\tdXNlcg==\tYw==\tYyAtLWY=\n",
            ),
        );
        let service = ProcessService::new(registry);
        let task = service
            .start_process_monitoring(
                "session-1".to_string(),
                host(),
                verifier(),
                app.handle().clone(),
            )
            .unwrap();

        wait_until(|| service.get_process_status("session-1").is_some());
        assert_eq!(
            service.get_process_status("session-1").unwrap().total_count,
            1
        );
        assert!(service.stop_process_monitoring(app.handle(), &task.task_id));
    }

    #[test]
    fn process_worker_error_maps_to_process_error() {
        let app = mock_app();
        let registry = SharedExecRegistry::new();
        registry.insert("session-1", FakeEntry("PLATFORM=linux"));
        let service = ProcessService::new(registry);
        let failed = Arc::new(AtomicUsize::new(0));
        let failed_ref = failed.clone();
        app.listen("task:status", move |event| {
            let payload: serde_json::Value = serde_json::from_str(event.payload()).unwrap();
            if payload["status"] == "Failed" {
                assert_eq!(payload["error"]["code"], "ProcessError");
                failed_ref.fetch_add(1, Ordering::Release);
            }
        });

        service
            .start_process_monitoring(
                "session-1".to_string(),
                host(),
                verifier(),
                app.handle().clone(),
            )
            .unwrap();
        wait_until(|| failed.load(Ordering::Acquire) == 1);
    }
}
