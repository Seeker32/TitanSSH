#[cfg(test)]
mod service_tests {
    use crate::core::host_identity::HostKeyVerifier;
    use crate::core::monitor_service::*;
    use crate::models::host::{AuthType, HostConfig};
    use std::sync::Arc;

    /// 构建总是放行的主机身份校验器，供监控服务测试使用。
    fn test_allow_all_verifier() -> HostKeyVerifier {
        Arc::new(|_presented| Ok(()))
    }

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
            group: String::new(),
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

        let result = service.start_monitoring(
            "session-1".to_string(),
            host,
            test_allow_all_verifier(),
            app.handle().clone(),
        );

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
            .start_monitoring(
                "session-1".to_string(),
                make_host(),
                test_allow_all_verifier(),
                app.handle().clone(),
            )
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
            .start_monitoring(
                "session-1".to_string(),
                make_host(),
                test_allow_all_verifier(),
                app.handle().clone(),
            )
            .unwrap();
        service.stop_monitoring(&task.task_id);
        // 任务已从 HashMap 移除
        let tasks = service.tasks.lock().unwrap();
        assert!(!tasks.contains_key(&task.task_id));
    }

    // ─── 任务状态迁移权威化测试 ──────────────────────────────────────────────

    /// 构造已注册的测试任务句柄。
    fn insert_task(service: &MonitorService, task_id: &str, session_id: &str, status: TaskStatus) {
        service.tasks.lock().unwrap().insert(
            task_id.to_string(),
            MonitorTaskHandle {
                task_info: TaskInfo {
                    task_id: task_id.to_string(),
                    task_type: "monitor".to_string(),
                    session_id: Some(session_id.to_string()),
                    status,
                    created_at: 1_710_000_000_000,
                },
                shutdown: Arc::new(AtomicBool::new(false)),
            },
        );
    }

    /// 迁移必须先把 registry 更新到新状态，再发布一次事件。
    #[test]
    fn transition_updates_registry_then_emits_event() {
        use std::sync::atomic::AtomicUsize;
        use tauri::Listener;
        use tauri::test::mock_app;

        let app = mock_app();
        let service = MonitorService::new();
        insert_task(&service, "task-1", "session-1", TaskStatus::Pending);
        let emitted = Arc::new(AtomicUsize::new(0));
        let emitted_ref = emitted.clone();
        app.listen("task:status", move |_| {
            emitted_ref.fetch_add(1, Ordering::Relaxed);
        });

        assert!(transition_task_status(
            &service.tasks,
            &app.handle(),
            "task-1",
            TaskStatus::Running,
            None
        ));
        assert_eq!(
            service
                .tasks
                .lock()
                .unwrap()
                .get("task-1")
                .unwrap()
                .task_info
                .status,
            TaskStatus::Running
        );
        assert_eq!(emitted.load(Ordering::Relaxed), 1);
    }

    /// Failed 为终态：worker 返回后再尝试转 Done 必须被拒绝，且不得再发事件。
    #[test]
    fn failed_task_never_transitions_to_done() {
        use std::sync::atomic::AtomicUsize;
        use tauri::Listener;
        use tauri::test::mock_app;

        let app = mock_app();
        let service = MonitorService::new();
        insert_task(&service, "task-1", "session-1", TaskStatus::Running);
        let emitted = Arc::new(AtomicUsize::new(0));
        let emitted_ref = emitted.clone();
        app.listen("task:status", move |_| {
            emitted_ref.fetch_add(1, Ordering::Relaxed);
        });

        assert!(transition_task_status(
            &service.tasks,
            &app.handle(),
            "task-1",
            TaskStatus::Failed,
            Some(AppErrorInfo {
                code: "MonitorError".to_string(),
                detail: None,
                detail_key: Some("监控采集失败: {0}".to_string()),
                detail_params: Some(vec!["boom".to_string()]),
            })
        ));
        assert_eq!(emitted.load(Ordering::Relaxed), 1);

        // 模拟 worker 返回后无条件补发 Done 的旧路径
        assert!(!transition_task_status(
            &service.tasks,
            &app.handle(),
            "task-1",
            TaskStatus::Done,
            None
        ));
        assert_eq!(
            service
                .tasks
                .lock()
                .unwrap()
                .get("task-1")
                .unwrap()
                .task_info
                .status,
            TaskStatus::Failed,
            "Failed 后 registry 不得被覆盖为 Done"
        );
        assert_eq!(
            emitted.load(Ordering::Relaxed),
            1,
            "Failed 后不得再发 Done 事件"
        );
    }

    /// registry 中不存在的任务（已被 stop 移除）迁移被拒绝且不发事件。
    #[test]
    fn transition_rejected_for_unknown_task_emits_nothing() {
        use std::sync::atomic::AtomicUsize;
        use tauri::Listener;
        use tauri::test::mock_app;

        let app = mock_app();
        let service = MonitorService::new();
        let emitted = Arc::new(AtomicUsize::new(0));
        let emitted_ref = emitted.clone();
        app.listen("task:status", move |_| {
            emitted_ref.fetch_add(1, Ordering::Relaxed);
        });

        assert!(!transition_task_status(
            &service.tasks,
            &app.handle(),
            "ghost-task",
            TaskStatus::Done,
            None
        ));
        assert_eq!(emitted.load(Ordering::Relaxed), 0);
    }

    /// Pending 直接 Failed 不属于合法迁移：worker 必须先 Running 再失败。
    #[test]
    fn pending_to_failed_is_rejected() {
        use tauri::test::mock_app;

        let app = mock_app();
        let service = MonitorService::new();
        insert_task(&service, "task-1", "session-1", TaskStatus::Pending);

        assert!(!transition_task_status(
            &service.tasks,
            &app.handle(),
            "task-1",
            TaskStatus::Failed,
            Some(AppErrorInfo {
                code: "MonitorError".to_string(),
                detail: None,
                detail_key: Some("不应直接失败".to_string()),
                detail_params: None,
            })
        ));
        assert_eq!(
            service
                .tasks
                .lock()
                .unwrap()
                .get("task-1")
                .unwrap()
                .task_info
                .status,
            TaskStatus::Pending
        );
    }

    /// stop 后任务从 registry 移除：worker 迟到的 Done 迁移被拒绝且不发事件。
    #[test]
    fn stopped_task_suppresses_late_terminal_transition() {
        use std::sync::atomic::AtomicUsize;
        use tauri::Listener;
        use tauri::test::mock_app;

        let app = mock_app();
        let service = MonitorService::new();
        insert_task(&service, "task-1", "session-1", TaskStatus::Running);
        let emitted = Arc::new(AtomicUsize::new(0));
        let emitted_ref = emitted.clone();
        app.listen("task:status", move |_| {
            emitted_ref.fetch_add(1, Ordering::Relaxed);
        });

        service.stop_monitoring("task-1");

        assert!(!transition_task_status(
            &service.tasks,
            &app.handle(),
            "task-1",
            TaskStatus::Done,
            None
        ));
        assert_eq!(emitted.load(Ordering::Relaxed), 0);
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
                    cpu_usage: Some(10.0),
                    memory_usage: Some(20.0),
                    disk_usage: Some(30.0),
                    disk_available_bytes: Some(40),
                    disk_total_bytes: Some(50),
                    network: crate::models::monitor::NetworkSnapshot {
                        available: true,
                        interfaces: vec![],
                    },
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
