#[cfg(test)]
mod tests {
    use crate::core::host_identity::HostKeyVerifier;
    use crate::core::process_service::{ProcessService, ProcessTaskHandle};
    use crate::core::shared_exec_registry::{ExecConnectionEntry, SharedExecRegistry};
    use crate::core::ssh_transport::ExecTransport;
    use crate::core::ssh_transport::test_support::repeating_exec;
    use crate::errors::app_error::AppError;
    use crate::models::host::{AuthType, HostConfig};
    use crate::models::monitor::{TaskInfo, TaskStatus};
    use crate::models::process::{ProcessInfo, ProcessSnapshot};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};
    use tauri::Listener;
    use tauri::test::mock_app;

    struct FakeEntry(String);

    impl ExecConnectionEntry for FakeEntry {
        /// 返回固定输出的 mock exec capability。
        fn exec_transport(&self) -> ExecTransport {
            repeating_exec(self.0.clone())
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

    fn output() -> String {
        "PLATFORM=linux\nHZ=100\nP\t1\t0\tR\t10\t5\t4096\tdXNlcg==\tYw==\tYyAtLWY=\n".to_string()
    }

    fn snapshot(session_id: &str) -> ProcessSnapshot {
        ProcessSnapshot {
            session_id: session_id.to_string(),
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
        }
    }

    #[test]
    fn start_emits_pending_running_and_process_snapshot() {
        let app = mock_app();
        let registry = SharedExecRegistry::new();
        registry.insert("session-1", FakeEntry(output()));
        let service = ProcessService::new(registry);
        let statuses = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
        let statuses_ref = statuses.clone();
        let snapshots = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
        let snapshots_ref = snapshots.clone();
        app.listen("task:status", move |event| {
            statuses_ref
                .lock()
                .unwrap()
                .push(serde_json::from_str(event.payload()).unwrap());
        });
        app.listen("process:snapshot", move |event| {
            snapshots_ref
                .lock()
                .unwrap()
                .push(serde_json::from_str(event.payload()).unwrap());
        });

        let task = service
            .start_process_monitoring(
                "session-1".to_string(),
                host(),
                verifier(),
                app.handle().clone(),
            )
            .unwrap();
        assert_eq!(task.status, TaskStatus::Pending);

        let deadline = Instant::now() + Duration::from_secs(2);
        while service.get_process_status("session-1").is_none() {
            assert!(Instant::now() < deadline, "process snapshot should arrive");
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(snapshots.lock().unwrap()[0]["sessionId"], "session-1");
        assert_eq!(snapshots.lock().unwrap()[0]["totalCount"], 1);
        service.stop_process_monitoring(app.handle(), &task.task_id);

        let statuses = statuses.lock().unwrap();
        assert_eq!(statuses[0]["status"], "Pending");
        assert_eq!(statuses[1]["status"], "Running");
        assert_eq!(statuses.last().unwrap()["status"], "Done");
    }

    #[test]
    fn missing_password_fails_before_registering_task() {
        let app = mock_app();
        let service = ProcessService::new(SharedExecRegistry::new());
        let mut invalid_host = host();
        invalid_host.auth_type = AuthType::Password;
        invalid_host.password_ref = None;

        let result = service.start_process_monitoring(
            "session-1".to_string(),
            invalid_host,
            verifier(),
            app.handle().clone(),
        );

        assert!(matches!(result, Err(AppError::InvalidHostConfig(_))));
        assert!(service.tasks.lock().unwrap().is_empty());
    }

    #[test]
    fn teardown_tombstone_rejects_late_start() {
        let app = mock_app();
        let service = ProcessService::new(SharedExecRegistry::new());
        service.stop_session(app.handle(), "session-1");
        let result = service.start_process_monitoring(
            "session-1".to_string(),
            host(),
            verifier(),
            app.handle().clone(),
        );
        assert!(matches!(result, Err(AppError::SessionNotFound(_))));
        assert!(service.tasks.lock().unwrap().is_empty());
    }

    #[test]
    fn stop_session_discards_late_snapshot_and_stops_only_matching_task() {
        let app = mock_app();
        let service = ProcessService::new(SharedExecRegistry::new());
        let target_shutdown = Arc::new(AtomicBool::new(false));
        let other_shutdown = Arc::new(AtomicBool::new(false));
        for (task_id, session_id, shutdown) in [
            ("target-task", "target-session", target_shutdown.clone()),
            ("other-task", "other-session", other_shutdown.clone()),
        ] {
            service.tasks.lock().unwrap().insert(
                task_id.to_string(),
                ProcessTaskHandle {
                    task_info: TaskInfo {
                        task_id: task_id.to_string(),
                        task_type: "process".to_string(),
                        session_id: Some(session_id.to_string()),
                        status: TaskStatus::Running,
                        created_at: 0,
                    },
                    shutdown,
                },
            );
        }
        let target = snapshot("target-session");
        assert!(crate::core::process_service::apply_snapshot_if_task_alive(
            &service.tasks,
            &service.snapshots,
            app.handle(),
            &target_shutdown,
            "target-task",
            &target
        ));

        service.stop_session(app.handle(), "target-session");
        assert!(target_shutdown.load(Ordering::Acquire));
        assert!(!other_shutdown.load(Ordering::Acquire));
        assert!(service.get_process_status("target-session").is_none());
        assert!(!crate::core::process_service::apply_snapshot_if_task_alive(
            &service.tasks,
            &service.snapshots,
            app.handle(),
            &target_shutdown,
            "target-task",
            &target
        ));
        assert!(service.tasks.lock().unwrap().contains_key("other-task"));
    }

    #[test]
    fn panic_guard_fails_task_with_error() {
        let app = mock_app();
        let service = ProcessService::new(SharedExecRegistry::new());
        service.tasks.lock().unwrap().insert(
            "task-1".to_string(),
            ProcessTaskHandle {
                task_info: TaskInfo {
                    task_id: "task-1".to_string(),
                    task_type: "process".to_string(),
                    session_id: Some("session-1".to_string()),
                    status: TaskStatus::Running,
                    created_at: 0,
                },
                shutdown: Arc::new(AtomicBool::new(false)),
            },
        );
        let captured = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
        let captured_ref = captured.clone();
        app.listen("task:status", move |event| {
            captured_ref
                .lock()
                .unwrap()
                .push(serde_json::from_str(event.payload()).unwrap());
        });

        crate::core::process_service::run_loop_with_panic_guard(
            &service.tasks,
            app.handle(),
            "task-1",
            || panic!("worker panic"),
        );

        assert_eq!(
            service.tasks.lock().unwrap()["task-1"].task_info.status,
            TaskStatus::Failed
        );
        assert_eq!(captured.lock().unwrap()[0]["error"]["code"], "ProcessError");
    }

    #[test]
    fn snapshot_emit_failure_sets_shutdown_and_failed_status() {
        let app = mock_app();
        let service = ProcessService::new(SharedExecRegistry::new());
        let shutdown = Arc::new(AtomicBool::new(false));
        let events = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
        let events_ref = events.clone();
        app.listen("task:status", move |event| {
            events_ref
                .lock()
                .unwrap()
                .push(serde_json::from_str(event.payload()).unwrap());
        });
        service.tasks.lock().unwrap().insert(
            "task-1".to_string(),
            ProcessTaskHandle {
                task_info: TaskInfo {
                    task_id: "task-1".to_string(),
                    task_type: "process".to_string(),
                    session_id: Some("session-1".to_string()),
                    status: TaskStatus::Running,
                    created_at: 0,
                },
                shutdown: shutdown.clone(),
            },
        );
        crate::core::process_service::handle_snapshot_emit_failure(
            &shutdown,
            &service.tasks,
            app.handle(),
            "task-1",
            "event failure",
        );
        assert!(shutdown.load(Ordering::Acquire));
        assert_eq!(
            service.tasks.lock().unwrap()["task-1"].task_info.status,
            TaskStatus::Failed
        );
        assert_eq!(events.lock().unwrap()[0]["status"], "Failed");
        assert_eq!(events.lock().unwrap()[0]["error"]["code"], "ProcessError");
    }
}
