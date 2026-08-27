#[cfg(test)]
mod integration_tests {
    use crate::core::host_identity::{HostIdentityService, HostKeyVerifier, PresentedHostKey};
    use crate::core::terminal_service::*;
    use crate::errors::app_error::{AppError, ErrorDetail};
    use crate::models::host::{AuthType, HostConfig};
    use crate::models::session::SessionStatus;
    use serde_json::json;

    /// 构建总是放行的主机身份校验器，供不关注身份确认的终端测试使用。
    fn test_allow_all_verifier() -> HostKeyVerifier {
        Arc::new(|_presented: &PresentedHostKey| Ok(()))
    }

    /// 构造测试用 HostConfig（密码认证模式）
    fn make_password_host(password_ref: Option<&str>) -> HostConfig {
        HostConfig {
            id: "host-test".to_string(),
            name: "test".to_string(),
            host: "127.0.0.1".to_string(),
            port: 22,
            username: "root".to_string(),
            auth_type: AuthType::Password,
            password_ref: password_ref.map(|s| s.to_string()),
            private_key_path: None,
            passphrase_ref: None,
            remark: None,
            group: String::new(),
        }
    }

    /// 构造测试用 HostConfig（私钥认证模式）
    fn make_privkey_host(key_path: Option<&str>, passphrase_ref: Option<&str>) -> HostConfig {
        HostConfig {
            id: "host-test".to_string(),
            name: "test".to_string(),
            host: "127.0.0.1".to_string(),
            port: 22,
            username: "root".to_string(),
            auth_type: AuthType::PrivateKey,
            password_ref: None,
            private_key_path: key_path.map(|s| s.to_string()),
            passphrase_ref: passphrase_ref.map(|s| s.to_string()),
            remark: None,
            group: String::new(),
        }
    }

    /// 状态事件跨越 event seam 前必须先更新后端运行时状态。
    #[test]
    fn session_status_event_updates_backend_runtime_first() {
        use tauri::test::mock_app;

        let app = mock_app();
        let runtime_status = Arc::new(Mutex::new(SessionStatus::Connecting));
        emit_session_status(
            &app.handle().clone(),
            "session-1",
            &runtime_status,
            SessionStatus::Connected,
            None,
        );

        assert_eq!(
            *runtime_status
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
            SessionStatus::Connected
        );
    }

    /// 验证 load_credentials：密码认证模式下 password_ref 为 None 时返回 InvalidHostConfig 错误
    #[test]
    fn load_credentials_password_mode_missing_ref_returns_error() {
        let host = make_password_host(None);
        let result = load_credentials(&host);
        assert!(result.is_err(), "缺少 password_ref 时应返回错误");
        match result.unwrap_err() {
            AppError::InvalidHostConfig(msg) => {
                assert!(
                    msg.to_string().contains("密码"),
                    "错误消息应提及密码，实际: {}",
                    msg
                );
            }
            other => panic!("期望 InvalidHostConfig，实际: {:?}", other),
        }
    }

    /// 验证 load_credentials：私钥认证模式下 private_key_path 为 None 时返回 InvalidHostConfig 错误
    #[test]
    fn load_credentials_privkey_mode_missing_path_returns_error() {
        let host = make_privkey_host(None, None);
        let result = load_credentials(&host);
        assert!(result.is_err(), "缺少私钥路径时应返回错误");
        match result.unwrap_err() {
            AppError::InvalidHostConfig(msg) => {
                assert!(
                    msg.to_string().contains("私钥路径"),
                    "错误消息应提及私钥路径，实际: {}",
                    msg
                );
            }
            other => panic!("期望 InvalidHostConfig，实际: {:?}", other),
        }
    }

    /// 验证 load_credentials：私钥认证模式下无口令引用时返回 (None, None)
    /// 私钥口令为可选项，无引用时不应报错
    #[test]
    fn load_credentials_privkey_mode_no_passphrase_ref_returns_none() {
        let host = make_privkey_host(Some("~/.ssh/id_rsa"), None);
        let result = load_credentials(&host);
        // 无 passphrase_ref 时不调用 secure_store，直接返回 (None, None)
        assert!(result.is_ok(), "无口令引用时应成功，实际: {:?}", result);
        let (password, passphrase) = result.unwrap();
        assert!(password.is_none(), "私钥模式下 password 应为 None");
        assert!(passphrase.is_none(), "无口令引用时 passphrase 应为 None");
    }

    /// 验证认证错误映射：AuthenticationError → SessionStatus::AuthFailed
    #[test]
    fn auth_error_maps_to_auth_failed_status() {
        let error = AppError::AuthenticationError("wrong password".to_string().into());
        let (status, message) = map_phase_error_to_status(&ConnectionPhase::Authenticating, &error);
        assert_eq!(
            status,
            SessionStatus::AuthFailed,
            "认证错误应映射为 AuthFailed"
        );
        // 直接转发结构化错误：code 稳定，前端据此本地化“认证失败”摘要
        assert_eq!(
            message.unwrap().code,
            "AuthenticationError",
            "应转发原错误 code"
        );
    }

    /// 验证连接超时错误映射：SshConnectionError("Connection timeout") → SessionStatus::Timeout
    #[test]
    fn connection_timeout_error_maps_to_timeout_status() {
        let error = AppError::SshConnectionError("Connection timeout after 30s".to_string().into());
        let (status, message) = map_phase_error_to_status(&ConnectionPhase::ConnectingTcp, &error);
        assert_eq!(status, SessionStatus::Timeout, "超时错误应映射为 Timeout");
        // 阶段超时文案作为 detailKey 下发，前端按语言翻译
        let message = message.unwrap();
        assert_eq!(message.code, "Timeout");
        assert_eq!(message.detail_key.as_deref(), Some("建立 TCP 连接超时"));
    }

    /// 验证网络连接错误映射：SshConnectionError（非超时）→ SessionStatus::Error
    #[test]
    fn network_error_maps_to_error_status() {
        let error = AppError::SshConnectionError("Connection refused".to_string().into());
        let (status, message) = map_phase_error_to_status(&ConnectionPhase::ConnectingTcp, &error);
        assert_eq!(status, SessionStatus::Error, "网络错误应映射为 Error");
        assert_eq!(message.unwrap().code, "SshConnectionError");
    }

    /// 验证 SSH 协议错误映射为 SessionStatus::Error。
    #[test]
    fn ssh_protocol_error_maps_to_error_status() {
        // 使用 StorageError 模拟其他协议错误的映射路径。
        let error = AppError::StorageError("handshake failed".to_string().into());
        let (status, _message) = map_phase_error_to_status(&ConnectionPhase::SshHandshake, &error);
        assert_eq!(status, SessionStatus::Error, "其他错误应映射为 Error");
    }

    /// 验证不同 SshConnectionError 消息的超时判断边界
    #[test]
    fn connection_timeout_detection_accepts_multiple_message_shapes() {
        let timeout_err = AppError::SshConnectionError("Connection timeout".to_string().into());
        let (status, _) = map_phase_error_to_status(&ConnectionPhase::ConnectingTcp, &timeout_err);
        assert_eq!(status, SessionStatus::Timeout);

        let lower_case_err =
            AppError::SshConnectionError("connection timed out".to_string().into());
        let (status2, _) =
            map_phase_error_to_status(&ConnectionPhase::ConnectingTcp, &lower_case_err);
        assert_eq!(status2, SessionStatus::Timeout);

        let chinese_err =
            AppError::SshConnectionError(ErrorDetail::msg("网络连接超时", Vec::new()));
        let (status3, _) = map_phase_error_to_status(&ConnectionPhase::ConnectingTcp, &chinese_err);
        assert_eq!(status3, SessionStatus::Timeout);
    }

    /// 验证独立超时判断函数覆盖常见文案
    #[test]
    fn is_timeout_message_matches_common_timeout_text() {
        assert!(is_timeout_message("Connection timeout after 10s"));
        assert!(is_timeout_message("connection timed out"));
        assert!(is_timeout_message("连接超时"));
        assert!(!is_timeout_message("connection refused"));
    }

    /// 验证连接阶段事件序列化为 camelCase，符合前后端事件契约
    #[test]
    fn connection_progress_event_serializes_as_camel_case() {
        let event = ConnectionProgressEvent {
            session_id: "session-1".to_string(),
            phase: ConnectionPhase::LoadingCredentials,
            timestamp: 1_710_000_000_111,
        };

        let value = serde_json::to_value(&event).expect("事件序列化应成功");
        assert_eq!(
            value,
            json!({
                "sessionId": "session-1",
                "phase": "LoadingCredentials",
                "timestamp": 1_710_000_000_111_i64,
            })
        );
    }

    /// Transport 的 channel 初始化阶段必须保持现有 Terminal 诊断事件语义。
    #[test]
    fn transport_channel_phase_maps_to_terminal_progress() {
        assert_eq!(
            map_connect_phase(crate::core::ssh_transport::ConnectPhase::OpeningChannel),
            ConnectionPhase::OpeningChannel
        );
    }

    /// 验证凭据不存在错误映射：CredentialNotFound → SessionStatus::Error + 引导提示
    ///
    /// 区别于通用 SecureStoreError，CredentialNotFound 应给出明确的"重新保存"引导，
    /// 而不是让用户面对无意义的技术错误消息。引导文案由前端按 code 本地化，
    /// 后端只转发结构化错误与 key 诊断。
    #[test]
    fn credential_not_found_maps_to_error_with_guidance_message() {
        let key = "titanssh-host-abc-password";
        let error = AppError::CredentialNotFound(key.to_string().into());
        let (status, message) =
            map_phase_error_to_status(&ConnectionPhase::LoadingCredentials, &error);

        assert_eq!(status, SessionStatus::Error, "凭据不存在应映射为 Error");
        let message = message.unwrap();
        assert_eq!(message.code, "CredentialNotFound");
        assert_eq!(
            message.detail.as_deref(),
            Some(key),
            "应携带具体的 key 便于诊断"
        );
    }

    /// 验证 SecureStoreError（非超时）仍映射为通用 Error，不与 CredentialNotFound 混淆
    #[test]
    fn secure_store_error_non_timeout_maps_to_generic_error() {
        let error = AppError::SecureStoreError("keychain locked".to_string().into());
        let (status, message) =
            map_phase_error_to_status(&ConnectionPhase::LoadingCredentials, &error);

        assert_eq!(status, SessionStatus::Error, "安全存储错误应映射为 Error");
        assert_eq!(message.unwrap().code, "SecureStoreError");
    }

    /// 构建模拟 transport 顺序的连接函数：握手后、认证前调用统一校验器。
    /// 与生产 ssh_transport::connect_session 的校验位置一致。
    fn gated_connect_fn(presented: PresentedHostKey) -> TerminalConnectFn {
        Box::new(
            move |_host,
                  _password,
                  _passphrase,
                  verifier,
                  on_phase: &mut dyn FnMut(ConnectPhase)| {
                on_phase(ConnectPhase::ConnectingTcp);
                on_phase(ConnectPhase::SshHandshake);
                on_phase(ConnectPhase::VerifyingHostKey);
                verifier(&presented)?;
                on_phase(ConnectPhase::Authenticating);
                Ok(crate::core::ssh_transport::test_support::idle_terminal())
            },
        )
    }

    /// 等待后端权威状态偏离 Connecting，返回最终状态。
    fn wait_for_final_status(
        runtime_status: &Arc<Mutex<SessionStatus>>,
        timeout: Duration,
    ) -> SessionStatus {
        let deadline = Instant::now() + timeout;
        loop {
            let status = runtime_status
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone();
            if status != SessionStatus::Connecting || Instant::now() >= deadline {
                return status;
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    /// 凭据读取永久挂起（如系统钥匙串授权框被遮挡、系统无响应）时，
    /// 终端工作线程必须在独立的凭据预算内上报 Timeout 终态，
    /// 不得无限期停留 Connecting 让标签永远转圈。
    #[test]
    fn credential_loading_hang_emits_timeout_within_budget() {
        use std::sync::mpsc;
        use tauri::test::mock_app;

        let app = mock_app();
        let (_command_tx, command_rx) = mpsc::channel();
        let shutdown = Arc::new(AtomicBool::new(false));
        let runtime_status = Arc::new(Mutex::new(SessionStatus::Connecting));
        // 永不返回的凭据加载器：模拟钥匙串授权挂起
        let (_hang_tx, hang_rx) = mpsc::channel::<()>();

        start_terminal_session_with_parts(
            app.handle().clone(),
            make_password_host(Some("ref")),
            "session-credential-hang".to_string(),
            command_rx,
            shutdown,
            runtime_status.clone(),
            move |_| {
                // 阻塞直到发送端波 drop（未调用时永远挂起）
                let _ = hang_rx.recv();
                Ok((Some("password".to_string()), None))
            },
            test_allow_all_verifier(),
            Box::new(|_host, _password, _passphrase, _verifier, _on_phase| {
                panic!("凭据挂起期间连接函数不应被调用")
            }),
            Duration::from_millis(200),
            Duration::from_millis(200),
            Box::new(|| {}),
        );

        let status = wait_for_final_status(&runtime_status, Duration::from_secs(2));
        assert_ne!(
            status,
            SessionStatus::Connecting,
            "凭据读取挂起必须在有限预算内离开 Connecting"
        );
        assert_eq!(
            status,
            SessionStatus::Timeout,
            "凭据读取挂起应上报 Timeout（文案：读取系统凭据超时）"
        );
    }

    /// 凭据挂起期间会话被关闭时，终端工作线程必须在短轮询周期内退出，
    /// 不得等完整个凭据预算（关闭可取消性）。
    #[test]
    fn shutdown_during_credential_wait_exits_promptly() {
        use std::sync::mpsc;
        use tauri::test::mock_app;

        let app = mock_app();
        let (_command_tx, command_rx) = mpsc::channel();
        let shutdown = Arc::new(AtomicBool::new(false));
        let runtime_status = Arc::new(Mutex::new(SessionStatus::Connecting));
        let (_hang_tx, hang_rx) = mpsc::channel::<()>();
        let (exit_tx, exit_rx) = mpsc::channel();

        start_terminal_session_with_parts(
            app.handle().clone(),
            make_password_host(Some("ref")),
            "session-shutdown-during-credentials".to_string(),
            command_rx,
            shutdown.clone(),
            runtime_status,
            move |_| {
                let _ = hang_rx.recv();
                Ok((Some("password".to_string()), None))
            },
            test_allow_all_verifier(),
            Box::new(|_host, _password, _passphrase, _verifier, _on_phase| {
                panic!("凭据挂起期间连接函数不应被调用")
            }),
            Duration::from_secs(60),
            Duration::from_secs(5),
            Box::new(move || {
                let _ = exit_tx.send(());
            }),
        );

        shutdown.store(true, Ordering::Relaxed);
        exit_rx
            .recv_timeout(Duration::from_millis(500))
            .expect("凭据挂起期间关闭应立即退出终端工作线程");
    }

    /// 凭据加载线程 panic 时，终端工作线程必须上报 Error 终态，
    /// 不得静默退出让前端永远停留在 Connecting。
    #[test]
    fn credential_loader_panic_emits_error_status() {
        use std::sync::mpsc;
        use tauri::test::mock_app;

        let app = mock_app();
        let (_command_tx, command_rx) = mpsc::channel();
        let shutdown = Arc::new(AtomicBool::new(false));
        let runtime_status = Arc::new(Mutex::new(SessionStatus::Connecting));

        start_terminal_session_with_parts(
            app.handle().clone(),
            make_password_host(Some("ref")),
            "session-credential-panic".to_string(),
            command_rx,
            shutdown,
            runtime_status.clone(),
            |_| panic!("模拟凭据加载线程崩溃"),
            test_allow_all_verifier(),
            Box::new(|_host, _password, _passphrase, _verifier, _on_phase| {
                panic!("凭据崩溃后连接函数不应被调用")
            }),
            Duration::from_secs(5),
            Duration::from_secs(5),
            Box::new(|| {}),
        );

        let status = wait_for_final_status(&runtime_status, Duration::from_secs(2));
        assert_eq!(
            status,
            SessionStatus::Error,
            "凭据加载线程 panic 必须上报 Error，不得永远 Connecting"
        );
    }

    /// 关闭发生在连接等待期间时，终端工作线程必须在下一次短轮询后退出，
    /// 而不是继续等待连接函数结束或消耗完整连接超时预算。
    #[test]
    fn shutdown_during_connect_wait_exits_terminal_worker_promptly() {
        use std::sync::mpsc;
        use tauri::test::mock_app;

        let app = mock_app();
        let (_command_tx, command_rx) = mpsc::channel();
        let shutdown = Arc::new(AtomicBool::new(false));
        let runtime_status = Arc::new(Mutex::new(SessionStatus::Connecting));
        let (connect_started_tx, connect_started_rx) = mpsc::channel();
        let (exit_tx, exit_rx) = mpsc::channel();

        start_terminal_session_with_parts(
            app.handle().clone(),
            make_password_host(Some("ref")),
            "session-shutdown-during-connect".to_string(),
            command_rx,
            shutdown.clone(),
            runtime_status,
            |_| Ok((Some("password".to_string()), None)),
            test_allow_all_verifier(),
            Box::new(move |_host, _password, _passphrase, _verifier, on_phase| {
                on_phase(ConnectPhase::ConnectingTcp);
                let _ = connect_started_tx.send(());
                thread::sleep(Duration::from_secs(2));
                Err(AppError::SshConnectionError(
                    "connection refused".to_string().into(),
                ))
            }),
            Duration::from_secs(5),
            Duration::from_secs(5),
            Box::new(move || {
                let _ = exit_tx.send(());
            }),
        );

        connect_started_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("连接工作线程应已开始");
        shutdown.store(true, Ordering::Relaxed);

        exit_rx
            .recv_timeout(Duration::from_millis(500))
            .expect("关闭期间终端工作线程应立即退出");
    }

    /// 连接等待期间收到 Close 命令时，终端工作线程必须立即退出，且不得为已关闭
    /// 的 Session 发布 Connected 状态。
    #[test]
    fn close_command_during_connect_wait_exits_without_connected_status() {
        use std::sync::mpsc;
        use tauri::test::mock_app;

        let app = mock_app();
        let (command_tx, command_rx) = mpsc::channel();
        let shutdown = Arc::new(AtomicBool::new(false));
        let runtime_status = Arc::new(Mutex::new(SessionStatus::Connecting));
        let (connect_started_tx, connect_started_rx) = mpsc::channel();
        let (exit_tx, exit_rx) = mpsc::channel();

        start_terminal_session_with_parts(
            app.handle().clone(),
            make_password_host(Some("ref")),
            "session-close-during-connect".to_string(),
            command_rx,
            shutdown,
            runtime_status.clone(),
            |_| Ok((Some("password".to_string()), None)),
            test_allow_all_verifier(),
            Box::new(move |_host, _password, _passphrase, _verifier, on_phase| {
                on_phase(ConnectPhase::ConnectingTcp);
                let _ = connect_started_tx.send(());
                thread::sleep(Duration::from_secs(2));
                Ok(crate::core::ssh_transport::test_support::idle_terminal())
            }),
            Duration::from_secs(5),
            Duration::from_secs(5),
            Box::new(move || {
                let _ = exit_tx.send(());
            }),
        );

        connect_started_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("连接工作线程应已开始");
        command_tx
            .send(TerminalCommand::Close)
            .expect("关闭命令应可发送到终端工作线程");

        exit_rx
            .recv_timeout(Duration::from_millis(500))
            .expect("连接等待期间收到 Close 后终端工作线程应立即退出");
        assert_eq!(
            *runtime_status
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
            SessionStatus::Connecting,
            "已关闭的 Session 不得在连接完成前发布 Connected 状态"
        );
    }

    /// 写入失败意味着终端通道已失效：工作线程必须只派发一次 Disconnected 并退出，
    /// 不得保留在循环中为后续按键重复派发 Error。
    #[test]
    fn write_failure_disconnects_and_exits_terminal_worker() {
        use std::sync::mpsc;
        use tauri::test::mock_app;

        let app = mock_app();
        let statuses = Arc::new(Mutex::new(Vec::new()));
        let statuses_for_listener = statuses.clone();
        {
            use tauri::Listener;
            app.listen("session:status", move |event| {
                let payload: serde_json::Value =
                    serde_json::from_str(event.payload()).expect("状态事件应为结构化数据");
                statuses_for_listener
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .push(
                        payload["status"]
                            .as_str()
                            .expect("状态事件必须包含字符串 status 字段")
                            .to_string(),
                    );
            });
        }

        let (command_tx, command_rx) = mpsc::channel();
        let shutdown = Arc::new(AtomicBool::new(false));
        let runtime_status = Arc::new(Mutex::new(SessionStatus::Connecting));
        let (exit_tx, exit_rx) = mpsc::channel();

        start_terminal_session_with_parts(
            app.handle().clone(),
            make_password_host(Some("ref")),
            "session-write-failure".to_string(),
            command_rx,
            shutdown,
            runtime_status.clone(),
            |_| Ok((Some("password".to_string()), None)),
            test_allow_all_verifier(),
            Box::new(|_host, _password, _passphrase, _verifier, _on_phase| {
                Ok(crate::core::ssh_transport::test_support::write_failing_terminal())
            }),
            Duration::from_secs(1),
            Duration::from_secs(1),
            Box::new(move || {
                let _ = exit_tx.send(());
            }),
        );

        command_tx
            .send(TerminalCommand::Write(b"echo test\n".to_vec()))
            .expect("写入命令应可发送到终端工作线程");
        exit_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("写入失败后终端工作线程应退出");

        assert_eq!(
            *runtime_status
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
            SessionStatus::Disconnected,
            "写入失败后会话应进入 Disconnected 状态"
        );
        assert_eq!(
            *statuses
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
            vec!["Connected", "Disconnected"],
            "写入失败不得派发 Error 或留下可重复派发状态的工作线程"
        );
        assert!(
            command_tx
                .send(TerminalCommand::Write(b"ignored\n".to_vec()))
                .is_err(),
            "工作线程退出后不应继续接收写入命令"
        );
    }

    /// 终端输出中的 UTF-8 字符跨两次底层读取时，terminal:data 事件合并后必须与
    /// 原始文本完全一致，且不得产生替换字符。
    #[test]
    fn terminal_data_events_preserve_utf8_character_split_across_reads() {
        use std::sync::mpsc;
        use tauri::test::mock_app;

        let app = mock_app();
        let received_data = Arc::new(Mutex::new(Vec::new()));
        let received_data_for_listener = received_data.clone();
        {
            use tauri::Listener;
            app.listen("terminal:data", move |event| {
                let payload: serde_json::Value =
                    serde_json::from_str(event.payload()).expect("终端事件应为结构化数据");
                received_data_for_listener
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .push(
                        payload["data"]
                            .as_str()
                            .expect("终端事件必须包含字符串 data 字段")
                            .to_string(),
                    );
            });
        }

        let (_command_tx, command_rx) = mpsc::channel();
        let shutdown = Arc::new(AtomicBool::new(false));
        let runtime_status = Arc::new(Mutex::new(SessionStatus::Connecting));
        let (exit_tx, exit_rx) = mpsc::channel();
        // 使 4096 字节读取恰好以 "中" 的前两个 UTF-8 字节结束，模拟生产缓冲区边界。
        let mut first_chunk = vec![b'x'; 4094];
        first_chunk.extend_from_slice(b"\xE4\xB8");
        let expected = format!("{}中B", "x".repeat(4094));

        start_terminal_session_with_parts(
            app.handle().clone(),
            make_password_host(Some("ref")),
            "session-split-utf8".to_string(),
            command_rx,
            shutdown,
            runtime_status,
            |_| Ok((Some("password".to_string()), None)),
            test_allow_all_verifier(),
            Box::new(move |_host, _password, _passphrase, _verifier, _on_phase| {
                // "中" 的 UTF-8 编码为 E4 B8 AD，故该字符被刻意拆在两个读取块之间。
                Ok(crate::core::ssh_transport::test_support::chunked_terminal(
                    vec![first_chunk, b"\xADB".to_vec()],
                ))
            }),
            Duration::from_secs(1),
            Duration::from_secs(1),
            Box::new(move || {
                let _ = exit_tx.send(());
            }),
        );

        exit_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("预设终端输出读取结束后工作线程应退出");

        let data = received_data
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .join("");
        assert_eq!(data, expected, "跨读取边界的 UTF-8 字符不得损坏");
        assert!(
            !data.contains('\u{FFFD}'),
            "terminal:data 事件不得包含 UTF-8 替换字符"
        );
    }

    /// 主机身份等待用户决定期间不占用连接总超时：远超预算仍保持 Connecting，
    /// 接受后进入认证并连接成功。
    #[test]
    fn host_identity_wait_does_not_consume_connect_timeout() {
        use tauri::test::mock_app;

        let app = mock_app();
        let identity = HostIdentityService::new();
        let (_command_tx, command_rx) = mpsc::channel();
        let shutdown = Arc::new(AtomicBool::new(false));
        let runtime_status = Arc::new(Mutex::new(SessionStatus::Connecting));

        start_terminal_session_with_parts(
            app.handle().clone(),
            make_password_host(Some("ref")),
            "session-identity-wait".to_string(),
            command_rx,
            shutdown,
            runtime_status.clone(),
            |_| Ok((Some("password".to_string()), None)),
            identity.verifier(app.handle().clone(), "session-identity-wait".to_string()),
            gated_connect_fn(PresentedHostKey {
                host: "10.0.0.8".to_string(),
                port: 22,
                algorithm: "ssh-ed25519".to_string(),
                fingerprint: "SHA256:terminal-wait".to_string(),
                blob: b"blob".to_vec(),
            }),
            // 预算远小于下方等待时长：验证等待期间不设独立自动超时
            Duration::from_millis(300),
            Duration::from_millis(300),
            Box::new(|| {}),
        );

        // challenge 出现后等待 1s（> 3× 预算），状态必须仍为 Connecting
        let deadline = Instant::now() + Duration::from_secs(2);
        while identity
            .pending_challenge("session-identity-wait")
            .is_none()
            && Instant::now() < deadline
        {
            thread::sleep(Duration::from_millis(10));
        }
        let challenge = identity
            .pending_challenge("session-identity-wait")
            .expect("终端连接产生主机身份 challenge");
        thread::sleep(Duration::from_millis(1_000));
        assert_eq!(
            wait_for_final_status(&runtime_status, Duration::from_millis(50)),
            SessionStatus::Connecting,
            "等待用户确认主机身份期间不设独立自动超时"
        );

        identity.accept(&challenge.challenge_id).unwrap();
        assert_eq!(
            wait_for_final_status(&runtime_status, Duration::from_secs(2)),
            SessionStatus::Connected,
            "仅本次接受后终端继续认证并连接成功"
        );
    }

    /// 预算在验证等待期间耗尽后接受：等待不消耗预算，连接完成优先于截止判定；
    /// 用户接受后会话必须继续认证而不是被立即判超时。
    #[test]
    fn accept_after_deadline_expired_during_verification_still_connects() {
        use tauri::test::mock_app;

        // 验证后继续认证需要一定时间（接受后 400ms 才完成连接），
        // 暴露"预算在等待期间耗尽"与"认证仍在进行"的交错：接受不得被立即判超时。
        let connect_fn: TerminalConnectFn = Box::new(
            move |_host,
                  _password,
                  _passphrase,
                  verifier,
                  on_phase: &mut dyn FnMut(ConnectPhase)| {
                on_phase(ConnectPhase::ConnectingTcp);
                on_phase(ConnectPhase::SshHandshake);
                on_phase(ConnectPhase::VerifyingHostKey);
                let presented = PresentedHostKey {
                    host: "10.0.0.8".to_string(),
                    port: 22,
                    algorithm: "ssh-ed25519".to_string(),
                    fingerprint: "SHA256:terminal-deadline".to_string(),
                    blob: b"blob".to_vec(),
                };
                verifier(&presented)?;
                on_phase(ConnectPhase::Authenticating);
                thread::sleep(Duration::from_millis(400));
                Ok(crate::core::ssh_transport::test_support::idle_terminal())
            },
        );

        let app = mock_app();
        let identity = HostIdentityService::new();
        let (_command_tx, command_rx) = mpsc::channel();
        let shutdown = Arc::new(AtomicBool::new(false));
        let runtime_status = Arc::new(Mutex::new(SessionStatus::Connecting));

        start_terminal_session_with_parts(
            app.handle().clone(),
            make_password_host(Some("ref")),
            "session-identity-deadline".to_string(),
            command_rx,
            shutdown,
            runtime_status.clone(),
            |_| Ok((Some("password".to_string()), None)),
            identity.verifier(
                app.handle().clone(),
                "session-identity-deadline".to_string(),
            ),
            connect_fn,
            // 预算远小于验证等待时长：预算在等待期间耗尽
            Duration::from_millis(300),
            Duration::from_millis(300),
            Box::new(|| {}),
        );

        // challenge 出现后等待超过预算，再接受
        let deadline = Instant::now() + Duration::from_secs(2);
        while identity
            .pending_challenge("session-identity-deadline")
            .is_none()
            && Instant::now() < deadline
        {
            thread::sleep(Duration::from_millis(10));
        }
        let challenge = identity
            .pending_challenge("session-identity-deadline")
            .expect("终端连接产生主机身份 challenge");
        thread::sleep(Duration::from_millis(1_000));

        identity.accept(&challenge.challenge_id).unwrap();
        assert_eq!(
            wait_for_final_status(&runtime_status, Duration::from_secs(2)),
            SessionStatus::Connected,
            "预算在等待期间耗尽，用户接受后会话仍应继续认证"
        );
    }

    /// 拒绝主机身份：终端连接失败，会话状态为 Error，不进入认证。
    #[test]
    fn host_identity_rejection_fails_terminal_as_error() {
        use tauri::test::mock_app;

        let app = mock_app();
        let identity = HostIdentityService::new();
        let (_command_tx, command_rx) = mpsc::channel();
        let shutdown = Arc::new(AtomicBool::new(false));
        let runtime_status = Arc::new(Mutex::new(SessionStatus::Connecting));

        start_terminal_session_with_parts(
            app.handle().clone(),
            make_password_host(Some("ref")),
            "session-identity-deny".to_string(),
            command_rx,
            shutdown,
            runtime_status.clone(),
            |_| Ok((Some("password".to_string()), None)),
            identity.verifier(app.handle().clone(), "session-identity-deny".to_string()),
            gated_connect_fn(PresentedHostKey {
                host: "10.0.0.8".to_string(),
                port: 22,
                algorithm: "ssh-ed25519".to_string(),
                fingerprint: "SHA256:terminal-deny".to_string(),
                blob: b"blob".to_vec(),
            }),
            Duration::from_secs(15),
            Duration::from_secs(15),
            Box::new(|| {}),
        );

        let deadline = Instant::now() + Duration::from_secs(2);
        while identity
            .pending_challenge("session-identity-deny")
            .is_none()
            && Instant::now() < deadline
        {
            thread::sleep(Duration::from_millis(10));
        }
        let challenge = identity.pending_challenge("session-identity-deny").unwrap();
        identity.reject(&challenge.challenge_id).unwrap();

        assert_eq!(
            wait_for_final_status(&runtime_status, Duration::from_secs(2)),
            SessionStatus::Error,
            "拒绝后终端连接以 Error 失败"
        );
    }

    /// 关闭 Session 取消等待中的主机身份验证：连接以取消错误退出，不进入认证。
    #[test]
    fn session_close_cancels_pending_host_identity_verification() {
        use tauri::test::mock_app;

        let app = mock_app();
        let identity = HostIdentityService::new();
        let (_command_tx, command_rx) = mpsc::channel();
        let shutdown = Arc::new(AtomicBool::new(false));
        let runtime_status = Arc::new(Mutex::new(SessionStatus::Connecting));

        start_terminal_session_with_parts(
            app.handle().clone(),
            make_password_host(Some("ref")),
            "session-identity-cancel".to_string(),
            command_rx,
            shutdown,
            runtime_status.clone(),
            |_| Ok((Some("password".to_string()), None)),
            identity.verifier(app.handle().clone(), "session-identity-cancel".to_string()),
            gated_connect_fn(PresentedHostKey {
                host: "10.0.0.8".to_string(),
                port: 22,
                algorithm: "ssh-ed25519".to_string(),
                fingerprint: "SHA256:terminal-cancel".to_string(),
                blob: b"blob".to_vec(),
            }),
            Duration::from_secs(15),
            Duration::from_secs(15),
            Box::new(|| {}),
        );

        let deadline = Instant::now() + Duration::from_secs(2);
        while identity
            .pending_challenge("session-identity-cancel")
            .is_none()
            && Instant::now() < deadline
        {
            thread::sleep(Duration::from_millis(10));
        }
        // 关闭 Session：取消全部等待者并清除临时信任
        identity.cancel_session(app.handle(), "session-identity-cancel");

        assert_eq!(
            wait_for_final_status(&runtime_status, Duration::from_secs(2)),
            SessionStatus::Error,
            "会话关闭取消等待中的主机身份验证，终端以 Error 退出"
        );
        assert!(
            identity
                .pending_challenge("session-identity-cancel")
                .is_none()
        );
    }

    /// 用户拒绝主机身份映射为 Error 状态并转发结构化语义。
    #[test]
    fn host_key_rejected_maps_to_error_status() {
        let (status, message) = map_phase_error_to_status(
            &ConnectionPhase::VerifyingHostKey,
            &AppError::HostKeyRejected("10.0.0.8:22 (SHA256:xxx)".to_string().into()),
        );
        assert_eq!(status, SessionStatus::Error);
        assert_eq!(message.unwrap().code, "HostKeyRejected");
    }

    /// 会话关闭取消的主机身份验证映射为 Error 状态并转发结构化语义。
    #[test]
    fn host_key_cancelled_maps_to_error_status() {
        let (status, message) = map_phase_error_to_status(
            &ConnectionPhase::VerifyingHostKey,
            &AppError::HostKeyVerificationCancelled("session-1".to_string().into()),
        );
        assert_eq!(status, SessionStatus::Error);
        assert_eq!(message.unwrap().code, "HostKeyVerificationCancelled");
    }

    /// 首次系统授权超过五秒后，成功读取的凭据仍应继续进入 SSH 连接阶段
    #[test]
    fn slow_credential_authorization_does_not_timeout_session() {
        use std::sync::mpsc;
        use std::time::{Duration, Instant};
        use tauri::test::mock_app;

        let app = mock_app();
        let mut host = make_password_host(Some("credential-ref"));
        host.port = 0;
        let (_command_tx, command_rx) = mpsc::channel();
        let shutdown = Arc::new(AtomicBool::new(false));
        let runtime_status = Arc::new(Mutex::new(SessionStatus::Connecting));

        start_terminal_session_with_parts(
            app.handle().clone(),
            host,
            "session-slow-authorization".to_string(),
            command_rx,
            shutdown,
            runtime_status.clone(),
            |_| {
                thread::sleep(Duration::from_millis(5_100));
                Ok((Some("password".to_string()), None))
            },
            test_allow_all_verifier(),
            Box::new(|_host, _password, _passphrase, _verifier, on_phase| {
                on_phase(ConnectPhase::ConnectingTcp);
                Err(AppError::SshConnectionError(
                    "connection refused".to_string().into(),
                ))
            }),
            Duration::from_secs(15),
            Duration::from_secs(15),
            Box::new(|| {}),
        );

        let deadline = Instant::now() + Duration::from_secs(7);
        while matches!(
            *runtime_status
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
            SessionStatus::Connecting
        ) && Instant::now() < deadline
        {
            thread::sleep(Duration::from_millis(10));
        }

        assert_eq!(
            *runtime_status
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
            SessionStatus::Error,
            "用户完成系统授权后，应继续进入 SSH 连接阶段并返回网络错误"
        );
    }
}
