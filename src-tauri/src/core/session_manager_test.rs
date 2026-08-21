#[cfg(test)]
mod tests {
    use crate::core::session_manager::*;
    use crate::core::shared_exec_registry::SharedExecRegistry;
    use crate::models::host::{AuthType, HostConfig};

    /// 创建共享运行时状态一致的 SessionManager。
    fn make_manager() -> SessionManager {
        SessionManager::new(
            MonitorService::new(SharedExecRegistry::new()),
            SftpService::new(),
            HostIdentityService::new(),
            SharedExecRegistry::new(),
        )
    }

    /// 构造测试用 HostConfig
    #[allow(dead_code)]
    fn make_host(id: &str) -> HostConfig {
        HostConfig {
            id: id.to_string(),
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

    /// host_config 对不存在的 session_id 返回 SessionNotFound 错误
    #[test]
    fn host_config_unknown_session_returns_error() {
        let manager = make_manager();
        let result = manager.host_config("nonexistent");
        assert!(result.is_err(), "不存在的 session_id 应返回错误");
        match result.unwrap_err() {
            AppError::SessionNotFound(id) => assert_eq!(id.to_string(), "nonexistent"),
            other => panic!("期望 SessionNotFound，实际: {:?}", other),
        }
    }

    /// 终端工作线程已退出并释放命令接收端后，写入与调整尺寸必须报告会话不可用，
    /// 不得将 channel SendError 伪装为底层 IO 故障。
    #[test]
    fn terminal_commands_with_dropped_receiver_return_session_not_found() {
        let manager = make_manager();
        let session_id = "session-terminal-worker-exited";
        manager.insert_session_for_test(session_id, make_host("host-1"));

        for result in [
            manager.write_terminal(session_id, "echo test".to_string()),
            manager.resize_terminal(session_id, 120, 40),
        ] {
            match result.unwrap_err() {
                AppError::SessionNotFound(id) => assert_eq!(id.to_string(), session_id),
                other => panic!("期望 SessionNotFound，实际: {other:?}"),
            }
        }
    }

    /// 应用退出必须一次性回收全部 Session 与 SFTP capability；重复调用不得复活或遗留资源。
    #[test]
    fn shutdown_all_reaps_every_session_and_sftp_registration() {
        use tauri::test::mock_app;

        let app = mock_app();
        let sftp_service = SftpService::with_connector(|_, _| {
            Err(AppError::SshConnectionError("unused".to_string().into()))
        });
        let manager = SessionManager::new(
            MonitorService::new(SharedExecRegistry::new()),
            sftp_service.clone(),
            HostIdentityService::new(),
            SharedExecRegistry::new(),
        );
        for session_id in ["session-exit-a", "session-exit-b"] {
            manager.insert_session_for_test(session_id, make_host(session_id));
            sftp_service.register_session(session_id.to_string(), make_host(session_id));
        }

        manager.shutdown_all(app.handle());
        manager.shutdown_all(app.handle());

        assert!(manager.list_sessions().is_empty());
        assert!(!sftp_service.has_session("session-exit-a"));
        assert!(!sftp_service.has_session("session-exit-b"));
    }

    /// list_sessions 必须读取后端运行时状态，不依赖前端回写。
    #[test]
    fn list_sessions_reads_backend_runtime_status() {
        let manager = make_manager();
        let (command_tx, _command_rx) = mpsc::channel();
        let session_id = "session-runtime-status".to_string();

        manager.sessions.lock().unwrap().insert(
            session_id.clone(),
            SessionHandle {
                meta: SessionInfo {
                    session_id: session_id.clone(),
                    host_id: "host-1".to_string(),
                    host: "127.0.0.1".to_string(),
                    port: 22,
                    username: "root".to_string(),
                    status: SessionStatus::Connecting,
                    created_at: 1_710_000_000_000,
                },
                runtime_status: Arc::new(Mutex::new(SessionStatus::Connected)),
                command_tx,
                shutdown: Arc::new(AtomicBool::new(false)),
                host: make_host("host-1"),
            },
        );

        let sessions = manager.list_sessions();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].status, SessionStatus::Connected);
    }

    /// Session 打开返回前必须注册 SFTP 状态槽，真实连接在后台并行进行。
    #[test]
    fn open_session_registers_sftp_connection_slot() {
        use tauri::test::mock_app;

        let app = mock_app();
        let sftp_service = SftpService::with_connector(|_, _| {
            Err(AppError::SshConnectionError(
                "expected test failure".to_string().into(),
            ))
        });
        let manager = SessionManager::new(
            MonitorService::new(SharedExecRegistry::new()),
            sftp_service.clone(),
            HostIdentityService::new(),
            SharedExecRegistry::new(),
        );

        let session = manager
            .open_session(app.handle().clone(), make_host("host-1"))
            .unwrap();

        assert!(sftp_service.has_session(&session.session_id));
    }

    /// 终端连接在读取凭据阶段失败时，后端必须自行回收 Session 和 SFTP 状态，
    /// 不得依赖前端收到状态事件后再调用 close_session。
    #[test]
    fn terminal_startup_failure_reaps_session_and_sftp_registration() {
        use std::time::{Duration, Instant};
        use tauri::test::mock_app;

        let app = mock_app();
        let sftp_service = SftpService::with_connector(|_, _| {
            Err(AppError::SshConnectionError(
                "expected test failure".to_string().into(),
            ))
        });
        let manager = SessionManager::new(
            MonitorService::new(SharedExecRegistry::new()),
            sftp_service.clone(),
            HostIdentityService::new(),
            SharedExecRegistry::new(),
        );
        let mut host = make_host("host-terminal-startup-failure");
        host.password_ref = None;

        let session = manager.open_session(app.handle().clone(), host).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while !manager.list_sessions().is_empty() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }

        assert!(
            manager.list_sessions().is_empty(),
            "凭据加载失败后不得保留已终态 Session"
        );
        assert!(
            !sftp_service.has_session(&session.session_id),
            "凭据加载失败后必须清理该 Session 的 SFTP 注册"
        );
    }

    /// close_session 必须取消该 Session 等待中的主机身份验证并清除临时信任。
    #[test]
    fn close_session_cancels_host_identity_waiters() {
        use crate::core::host_identity::PresentedHostKey;
        use std::time::{Duration, Instant};
        use tauri::test::mock_app;

        let app = mock_app();
        let identity = HostIdentityService::new();
        let sftp_service = SftpService::with_connector(|_, _| {
            Err(AppError::SshConnectionError(
                "expected test failure".to_string().into(),
            ))
        });
        let manager = SessionManager::new(
            MonitorService::new(SharedExecRegistry::new()),
            sftp_service,
            identity.clone(),
            SharedExecRegistry::new(),
        );
        let session_id = "session-identity-close".to_string();
        let (command_tx, _command_rx) = mpsc::channel();

        manager.sessions.lock().unwrap().insert(
            session_id.clone(),
            SessionHandle {
                meta: SessionInfo {
                    session_id: session_id.clone(),
                    host_id: "host-1".to_string(),
                    host: "127.0.0.1".to_string(),
                    port: 22,
                    username: "root".to_string(),
                    status: SessionStatus::Connecting,
                    created_at: 1_710_000_000_000,
                },
                runtime_status: Arc::new(Mutex::new(SessionStatus::Connecting)),
                command_tx,
                shutdown: Arc::new(AtomicBool::new(false)),
                host: make_host("host-1"),
            },
        );

        // 模拟该 Session 的 capability 连接正在等待主机身份确认
        let verifier = identity.verifier(app.handle().clone(), session_id.clone());
        let waiter = std::thread::spawn(move || {
            verifier(&PresentedHostKey {
                host: "127.0.0.1".to_string(),
                port: 22,
                algorithm: "ssh-ed25519".to_string(),
                fingerprint: "SHA256:manager-close".to_string(),
                blob: b"blob".to_vec(),
            })
        });
        let deadline = Instant::now() + Duration::from_secs(2);
        while identity.pending_challenge(&session_id).is_none() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }

        manager
            .close_session(&session_id, &app.handle().clone())
            .unwrap();

        // 等待者以取消错误退出，不得进入认证
        let error = waiter.join().unwrap().unwrap_err();
        assert_eq!(error.code(), "HostKeyVerificationCancelled");
        assert!(identity.pending_challenge(&session_id).is_none());
    }

    /// close_session 必须在后端一次性停止 Terminal、Monitoring 并清理 File Transfer。
    #[test]
    fn close_session_tears_down_all_backend_work() {
        use crate::core::monitor_service::MonitorTaskHandle;
        use crate::models::monitor::{TaskInfo, TaskStatus};
        use tauri::test::mock_app;

        let app = mock_app();
        let monitor_service = MonitorService::new(SharedExecRegistry::new());
        let sftp_service = SftpService::with_connector(|_, _| {
            Err(AppError::SshConnectionError(
                "expected test failure".to_string().into(),
            ))
        });
        let manager = SessionManager::new(
            monitor_service.clone(),
            sftp_service.clone(),
            HostIdentityService::new(),
            SharedExecRegistry::new(),
        );
        let session_id = "session-close-all".to_string();
        let terminal_shutdown = Arc::new(AtomicBool::new(false));
        let monitor_shutdown = Arc::new(AtomicBool::new(false));
        let (command_tx, _command_rx) = mpsc::channel();

        manager.sessions.lock().unwrap().insert(
            session_id.clone(),
            SessionHandle {
                meta: SessionInfo {
                    session_id: session_id.clone(),
                    host_id: "host-1".to_string(),
                    host: "127.0.0.1".to_string(),
                    port: 22,
                    username: "root".to_string(),
                    status: SessionStatus::Connected,
                    created_at: 1_710_000_000_000,
                },
                runtime_status: Arc::new(Mutex::new(SessionStatus::Connected)),
                command_tx,
                shutdown: terminal_shutdown.clone(),
                host: make_host("host-1"),
            },
        );
        monitor_service.tasks.lock().unwrap().insert(
            "monitor-task".to_string(),
            MonitorTaskHandle {
                task_info: TaskInfo {
                    task_id: "monitor-task".to_string(),
                    task_type: "monitor".to_string(),
                    session_id: Some(session_id.clone()),
                    status: TaskStatus::Running,
                    created_at: 1_710_000_000_000,
                },
                shutdown: monitor_shutdown.clone(),
            },
        );
        sftp_service.register_session(session_id.clone(), make_host("host-1"));

        manager
            .close_session(&session_id, &app.handle().clone())
            .unwrap();

        assert!(terminal_shutdown.load(Ordering::Relaxed));
        assert!(monitor_shutdown.load(Ordering::Acquire));
        assert!(monitor_service.tasks.lock().unwrap().is_empty());
        assert!(!sftp_service.has_session(&session_id));
    }

    /// 带释放信号的内存共享连接条目，供 teardown 回收断言。
    struct DroppingEntry {
        dropped: std::sync::mpsc::Sender<()>,
    }

    impl crate::core::shared_exec_registry::ExecConnectionEntry for DroppingEntry {
        /// 派生固定输出 capability（teardown 测试不执行采集）。
        fn exec_transport(&self) -> crate::core::ssh_transport::ExecTransport {
            crate::core::ssh_transport::test_support::repeating_exec("METRIC=1".to_string())
        }
    }

    impl Drop for DroppingEntry {
        /// 最后一个引用消失时通知测试。
        fn drop(&mut self) {
            let _ = self.dropped.send(());
        }
    }

    /// session teardown 必须回收共享 exec 注册表中的连接：条目释放、无泄漏；
    /// 其他会话的连接不受影响。
    #[test]
    fn close_session_recycles_shared_exec_connection() {
        use crate::core::shared_exec_registry::SharedExecRegistry;
        use tauri::test::mock_app;

        let app = mock_app();
        let registry = SharedExecRegistry::new();
        let (dropped_tx, dropped_rx) = std::sync::mpsc::channel();
        let (other_tx, other_rx) = std::sync::mpsc::channel();

        // 预置目标会话与其他会话的共享连接
        registry
            .resolve("session-close-exec", || {
                Ok(DroppingEntry {
                    dropped: dropped_tx,
                })
            })
            .expect("预置目标会话连接应成功");
        registry
            .resolve("session-other-exec", || {
                Ok(DroppingEntry { dropped: other_tx })
            })
            .expect("预置其他会话连接应成功");

        let manager = SessionManager::new(
            MonitorService::new(registry.clone()),
            SftpService::with_connector(|_, _| {
                Err(AppError::SshConnectionError("unused".to_string().into()))
            }),
            HostIdentityService::new(),
            registry.clone(),
        );
        manager.insert_session_for_test("session-close-exec", make_host("host-1"));

        manager
            .close_session("session-close-exec", &app.handle().clone())
            .unwrap();

        assert!(
            !registry.contains("session-close-exec"),
            "teardown 后不得残留连接条目"
        );
        dropped_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("teardown 必须释放共享连接（无泄漏）");
        assert!(
            registry.contains("session-other-exec"),
            "回收不得影响其他会话的连接"
        );
        assert!(other_rx.try_recv().is_err(), "其他会话连接不得被释放");
    }

    /// 应用退出时除逐会话回收外，还需清空注册表：不在 sessions 索引中的
    /// 残留条目（如回收后才插入的迟来连接）也不得泄漏。
    #[test]
    fn shutdown_all_clears_leftover_shared_exec_entries() {
        use crate::core::shared_exec_registry::SharedExecRegistry;
        use tauri::test::mock_app;

        let app = mock_app();
        let registry = SharedExecRegistry::new();
        let (dropped_tx, dropped_rx) = std::sync::mpsc::channel();
        registry
            .resolve("session-orphan-exec", || {
                Ok(DroppingEntry {
                    dropped: dropped_tx,
                })
            })
            .expect("预置孤立条目应成功");

        let manager = SessionManager::new(
            MonitorService::new(registry.clone()),
            SftpService::with_connector(|_, _| {
                Err(AppError::SshConnectionError("unused".to_string().into()))
            }),
            HostIdentityService::new(),
            registry.clone(),
        );

        manager.shutdown_all(app.handle());

        assert!(!registry.contains("session-orphan-exec"));
        dropped_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("退出兜底必须清空全部残留条目");
    }
}
