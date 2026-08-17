#[cfg(test)]
mod tests {
    use crate::core::session_manager::*;
    use crate::core::terminal_service::TerminalCommand;
    use crate::models::host::{AuthType, HostConfig};

    /// 创建共享运行时状态一致的 SessionManager。
    fn make_manager() -> SessionManager {
        SessionManager::new(
            MonitorService::new(),
            SftpService::new(),
            HostIdentityService::new(),
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

    /// 会话管理器必须原样保留非 UTF-8 终端输入字节，直到 TerminalCommand 队列。
    #[test]
    fn terminal_input_preserves_non_utf8_bytes() {
        let manager = make_manager();
        let session_id = "session-terminal-bytes";
        let receiver =
            manager.insert_session_for_test_with_receiver(session_id, make_host("host-1"));
        let input = vec![0x00, 0xff, 0x1b, b'[', b'A'];

        manager
            .write_terminal(session_id, input.clone())
            .expect("终端输入应成功入队");

        match receiver.recv().expect("应收到终端写入命令") {
            TerminalCommand::Write(data) => assert_eq!(data, input),
            _ => panic!("期望 Write 命令"),
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
            manager.write_terminal(session_id, b"echo test".to_vec()),
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
            MonitorService::new(),
            sftp_service.clone(),
            HostIdentityService::new(),
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
            MonitorService::new(),
            sftp_service.clone(),
            HostIdentityService::new(),
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
            MonitorService::new(),
            sftp_service.clone(),
            HostIdentityService::new(),
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
        let manager = SessionManager::new(MonitorService::new(), sftp_service, identity.clone());
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
        let monitor_service = MonitorService::new();
        let sftp_service = SftpService::with_connector(|_, _| {
            Err(AppError::SshConnectionError(
                "expected test failure".to_string().into(),
            ))
        });
        let manager = SessionManager::new(
            monitor_service.clone(),
            sftp_service.clone(),
            HostIdentityService::new(),
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
}
