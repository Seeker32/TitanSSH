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

    /// stop_monitoring 设置关闭标志后任务从 HashMap 中移除，并返回 true 表示确实移除
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
        assert!(
            service.stop_monitoring(app.handle(), &task.task_id),
            "存在的任务 stop 应返回 true"
        );
        // 任务已从 HashMap 移除
        let tasks = service.tasks.lock().unwrap();
        assert!(!tasks.contains_key(&task.task_id));
    }

    /// 停止不存在的任务返回 false：调用方可区分「已停止」与「从未存在/早已消失」
    #[test]
    fn stop_monitoring_unknown_task_returns_false() {
        use tauri::test::mock_app;
        let app = mock_app();
        let service = MonitorService::new();
        assert!(
            !service.stop_monitoring(app.handle(), "task-never-existed"),
            "未知任务 stop 应返回 false"
        );
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

    /// stop 后任务从 registry 移除：worker 迟到的 Done 迁移被拒绝，且停止方
    /// 已补发终态事件（见 stop_monitoring_emits_terminal_done_event），
    /// 迟到迁移不得再发第二次事件。
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

        assert!(service.stop_monitoring(app.handle(), "task-1"));

        assert!(!transition_task_status(
            &service.tasks,
            &app.handle(),
            "task-1",
            TaskStatus::Done,
            None
        ));
        // 只有停止方补发的那一个终态事件；迟到迁移不得追加
        assert_eq!(emitted.load(Ordering::Relaxed), 1);
    }

    /// stop_session 只清理目标 Session 的任务与快照，不影响其他 Session。
    #[test]
    fn stop_session_cleans_only_matching_monitor_state() {
        use tauri::test::mock_app;
        let app = mock_app();
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

        service.stop_session(app.handle(), "target-session");

        assert!(target_shutdown.load(Ordering::Acquire));
        assert!(!other_shutdown.load(Ordering::Acquire));
        assert!(service.get_monitor_status("target-session").is_none());
        assert!(service.get_monitor_status("other-session").is_some());
        assert!(service.tasks.lock().unwrap().contains_key("other-task"));
    }

    /// 回归：stop 路径必须补发终态 Done 事件。移除句柄后 worker 的迟到 Done
    /// 迁移被拒绝（见 stopped_task_suppresses_late_terminal_transition），
    /// 若停止方不补发事件，前端永远停留在 Running，显示幽灵任务。
    #[test]
    fn stop_monitoring_emits_terminal_done_event() {
        use tauri::Listener;
        use tauri::test::mock_app;

        let app = mock_app();
        let service = MonitorService::new();
        insert_task(&service, "task-1", "session-1", TaskStatus::Running);
        let captured = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
        let captured_ref = captured.clone();
        app.listen("task:status", move |event| {
            captured_ref
                .lock()
                .unwrap()
                .push(serde_json::from_str(event.payload()).expect("payload 应为 JSON"));
        });

        assert!(service.stop_monitoring(app.handle(), "task-1"));
        assert!(!service.tasks.lock().unwrap().contains_key("task-1"));

        let events = captured.lock().unwrap();
        assert_eq!(events.len(), 1, "stop 必须恰好补发一个终态事件");
        assert_eq!(events[0]["taskId"], "task-1");
        assert_eq!(events[0]["status"], "Done");
    }

    /// Failed 已由 worker 播发过终态事件；stop 清理时不得重复补发 Done。
    #[test]
    fn stop_monitoring_failed_task_emits_no_duplicate_terminal_event() {
        use tauri::Listener;
        use tauri::test::mock_app;

        let app = mock_app();
        let service = MonitorService::new();
        insert_task(&service, "task-1", "session-1", TaskStatus::Failed);
        let captured = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
        let captured_ref = captured.clone();
        app.listen("task:status", move |event| {
            captured_ref
                .lock()
                .unwrap()
                .push(serde_json::from_str(event.payload()).expect("payload 应为 JSON"));
        });

        assert!(service.stop_monitoring(app.handle(), "task-1"));
        assert!(!service.tasks.lock().unwrap().contains_key("task-1"));
        assert!(
            captured.lock().unwrap().is_empty(),
            "Failed 任务不得重复补发 Done"
        );
    }

    /// Session teardown：每个被停止的监控任务补发 Done 终态事件，其他会话不受影响。
    #[test]
    fn stop_session_emits_done_for_each_stopped_task() {
        use tauri::Listener;
        use tauri::test::mock_app;

        let app = mock_app();
        let service = MonitorService::new();
        insert_task(&service, "task-1", "session-1", TaskStatus::Running);
        insert_task(&service, "task-2", "session-1", TaskStatus::Running);
        insert_task(&service, "task-other", "session-2", TaskStatus::Running);
        let captured = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
        let captured_ref = captured.clone();
        app.listen("task:status", move |event| {
            captured_ref
                .lock()
                .unwrap()
                .push(serde_json::from_str(event.payload()).expect("payload 应为 JSON"));
        });

        service.stop_session(app.handle(), "session-1");

        let tasks = service.tasks.lock().unwrap();
        assert!(!tasks.contains_key("task-1") && !tasks.contains_key("task-2"));
        assert!(tasks.contains_key("task-other"));
        drop(tasks);

        let events = captured.lock().unwrap();
        assert_eq!(events.len(), 2, "每个被停止任务恰好一个终态事件");
        let mut ids: Vec<&str> = events
            .iter()
            .map(|event| event["taskId"].as_str().expect("taskId 应为字符串"))
            .collect();
        ids.sort_unstable();
        assert_eq!(ids, vec!["task-1", "task-2"]);
        assert!(
            events.iter().all(|event| event["status"] == "Done"),
            "teardown 补发的终态必须是 Done"
        );
    }

    /// 构造最小监控快照。
    fn make_snapshot(session_id: &str) -> MonitorSnapshot {
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
        }
    }

    /// 回归：stop_session 清理后，在途 collect_once 的迟到快照回调不得把
    /// 快照重新插入缓存（get_monitor_status 会原样返回陈旧数据），也不得
    /// 推送事件复活前端投影。
    #[test]
    fn late_snapshot_after_stop_session_is_discarded() {
        use std::sync::atomic::AtomicUsize;
        use tauri::Listener;
        use tauri::test::mock_app;

        let app = mock_app();
        let service = MonitorService::new();
        insert_task(&service, "task-1", "session-1", TaskStatus::Running);
        let snapshot = make_snapshot("session-1");
        let shutdown = Arc::new(AtomicBool::new(false));
        let emitted = Arc::new(AtomicUsize::new(0));
        let emitted_ref = emitted.clone();
        app.listen("monitor:snapshot", move |_| {
            emitted_ref.fetch_add(1, Ordering::Relaxed);
        });

        // 任务存活：快照落缓存并推送事件
        assert!(apply_snapshot_if_task_alive(
            &service.tasks,
            &service.snapshots,
            app.handle(),
            &shutdown,
            "task-1",
            &snapshot
        ));
        assert_eq!(
            service.get_monitor_status("session-1"),
            Some(snapshot.clone())
        );
        assert_eq!(emitted.load(Ordering::Relaxed), 1);

        // teardown 移除任务与快照
        service.stop_session(app.handle(), "session-1");
        assert!(service.get_monitor_status("session-1").is_none());

        // 在途采集迟到：不得复活缓存、不得推送事件
        assert!(!apply_snapshot_if_task_alive(
            &service.tasks,
            &service.snapshots,
            app.handle(),
            &shutdown,
            "task-1",
            &snapshot
        ));
        assert!(service.get_monitor_status("session-1").is_none());
        assert_eq!(emitted.load(Ordering::Relaxed), 1);
    }

    /// 回归：monitor:snapshot 推送失败时任务进入 Failed 终态，必须同时设置
    /// 关闭标志终止采集循环；否则 run_monitor_loop 每 2 秒继续采集并重复
    /// 失败推送，SSH 连接、远端执行与缓存写入永不停止。
    #[test]
    fn snapshot_emit_failure_sets_shutdown_and_fails_task() {
        use tauri::Listener;
        use tauri::test::mock_app;

        let app = mock_app();
        let service = MonitorService::new();
        insert_task(&service, "task-1", "session-1", TaskStatus::Running);
        let shutdown = Arc::new(AtomicBool::new(false));
        let captured = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
        let captured_ref = captured.clone();
        app.listen("task:status", move |event| {
            captured_ref
                .lock()
                .unwrap()
                .push(serde_json::from_str(event.payload()).expect("payload 应为 JSON"));
        });

        handle_snapshot_emit_failure(
            &shutdown,
            &service.tasks,
            app.handle(),
            "task-1",
            "推送失败",
        );

        assert!(
            shutdown.load(Ordering::Acquire),
            "推送失败必须设置关闭标志终止采集循环"
        );
        assert_eq!(
            service
                .tasks
                .lock()
                .unwrap()
                .get("task-1")
                .unwrap()
                .task_info
                .status,
            TaskStatus::Failed
        );
        let events = captured.lock().unwrap();
        assert_eq!(events.len(), 1, "恰好一个终态事件");
        assert_eq!(events[0]["taskId"], "task-1");
        assert_eq!(events[0]["status"], "Failed");
    }

    /// 回归：worker 或回调 panic 时必须迁移 Failed，不得把任务卡死在
    /// Running 的幽灵状态（线程死亡后无人再发终态事件/清理句柄，
    /// 快照缓存也会停止更新）。
    #[test]
    fn monitor_loop_panic_transitions_task_to_failed() {
        use tauri::Listener;
        use tauri::test::mock_app;

        let app = mock_app();
        let service = MonitorService::new();
        insert_task(&service, "task-1", "session-1", TaskStatus::Running);
        let captured = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
        let captured_ref = captured.clone();
        app.listen("task:status", move |event| {
            captured_ref
                .lock()
                .unwrap()
                .push(serde_json::from_str(event.payload()).expect("payload 应为 JSON"));
        });

        run_loop_with_panic_guard(&service.tasks, app.handle(), "task-1", || {
            panic!("采集解析 panic");
        });

        assert_eq!(
            service
                .tasks
                .lock()
                .unwrap()
                .get("task-1")
                .unwrap()
                .task_info
                .status,
            TaskStatus::Failed
        );
        let events = captured.lock().unwrap();
        assert_eq!(events.len(), 1, "panic 后恰好一个终态事件");
        assert_eq!(events[0]["status"], "Failed");
        assert_eq!(events[0]["error"]["code"], "MonitorError");
    }

    /// 循环正常退出时照旧迁移 Done（防护不得改变既有语义）。
    #[test]
    fn monitor_loop_normal_exit_transitions_to_done() {
        use tauri::Listener;
        use tauri::test::mock_app;

        let app = mock_app();
        let service = MonitorService::new();
        insert_task(&service, "task-1", "session-1", TaskStatus::Running);
        let captured = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
        let captured_ref = captured.clone();
        app.listen("task:status", move |event| {
            captured_ref
                .lock()
                .unwrap()
                .push(serde_json::from_str(event.payload()).expect("payload 应为 JSON"));
        });

        run_loop_with_panic_guard(&service.tasks, app.handle(), "task-1", || {});

        assert_eq!(
            service
                .tasks
                .lock()
                .unwrap()
                .get("task-1")
                .unwrap()
                .task_info
                .status,
            TaskStatus::Done
        );
        let events = captured.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["status"], "Done");
    }

    /// 回归：持锁线程 panic 毒化互斥锁后，服务各入口不得再跟着 panic——
    /// 注册表/缓存无跨调用不变量，恢复内部值继续服务；否则一次 panic
    /// 让全部会话的监控功能永久瘫痪。
    #[test]
    fn service_entries_tolerate_poisoned_mutexes() {
        use tauri::test::mock_app;
        let app = mock_app();
        let service = MonitorService::new();
        insert_task(&service, "task-1", "session-1", TaskStatus::Running);
        service.insert_snapshot_for_test(make_snapshot("session-1"));

        // 模拟持锁线程 panic:分别毒化 tasks 与 snapshots
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let _guard = service.tasks.lock().unwrap();
                panic!("模拟持 tasks 锁的线程 panic");
            }))
            .is_err()
        );
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let _guard = service.snapshots.lock().unwrap();
                panic!("模拟持 snapshots 锁的线程 panic");
            }))
            .is_err()
        );

        // 毒化后各入口照常工作，不得再 panic
        assert!(service.stop_monitoring(app.handle(), "task-1"));
        assert!(!service.stop_monitoring(app.handle(), "task-1"));
        service.stop_session(app.handle(), "session-2");
        assert!(service.get_monitor_status("session-1").is_some());
        assert!(!apply_snapshot_if_task_alive(
            &service.tasks,
            &service.snapshots,
            app.handle(),
            &Arc::new(AtomicBool::new(false)),
            "task-1",
            &make_snapshot("session-1"),
        ));
        assert!(!transition_task_status(
            &service.tasks,
            app.handle(),
            "task-1",
            TaskStatus::Done,
            None
        ));
    }
}
