#[cfg(test)]
mod tests {
    use crate::core::sftp_service::*;
    use crate::core::ssh_transport::test_support::{
        blocking_read_sftp, blocking_sftp, channel_failing_transfer_sftp, drop_signal_sftp,
        empty_sftp, failing_channel_sftp, failing_read_sftp, failing_write_sftp, memory_sftp,
        path_not_found_sftp,
    };
    use crate::core::transfer_pool::{MAX_TRANSFER_CONNECTIONS_PER_SESSION, is_idle_expired};
    use crate::models::host::{AuthType, HostConfig};
    use std::sync::atomic::Ordering;
    use std::sync::{Arc, Barrier};
    use std::time::Duration;

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

    /// SFTP 控制连接与其他 capability 一样经过主机身份统一校验：
    /// 未知主机在认证前阻塞等待用户决定，接受后才交付可用连接。
    #[test]
    fn sftp_control_connection_waits_for_host_identity_decision() {
        use crate::core::host_identity::{HostIdentityService, PresentedHostKey};
        use std::time::Instant;
        use tauri::test::mock_app;

        let app = mock_app();
        let identity = HostIdentityService::new();
        let service = SftpService::with_verifying_connector(
            |host, role, verifier| {
                // 模拟 transport 顺序：握手后、认证前进入统一校验
                verifier(&PresentedHostKey {
                    host: host.host.clone(),
                    port: host.port,
                    algorithm: "ssh-ed25519".to_string(),
                    fingerprint: "SHA256:sftp-identity".to_string(),
                    blob: b"blob".to_vec(),
                })?;
                assert_eq!(role, SftpRole::Control, "Session 打开先建控制连接");
                Ok(empty_sftp())
            },
            TransferClock::system(),
            TRANSFER_IDLE_TIMEOUT,
        );
        service.register_session_with_verifier(
            "session-identity".to_string(),
            make_host(),
            identity.verifier(app.handle().clone(), "session-identity".to_string()),
        );

        // 控制连接阻塞在主机身份确认：challenge 已产生
        let deadline = Instant::now() + Duration::from_secs(2);
        while identity.pending_challenge("session-identity").is_none() && Instant::now() < deadline
        {
            std::thread::sleep(Duration::from_millis(10));
        }
        let challenge = identity
            .pending_challenge("session-identity")
            .expect("SFTP 连接产生主机身份 challenge");

        // 仅本次接受后控制连接交付，目录操作可用
        identity.accept(&challenge.challenge_id).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if service.list_dir("session-identity", "/").is_ok() {
                break;
            }
            assert!(Instant::now() < deadline, "接受后 SFTP 控制连接应可用");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// 拒绝后 SFTP 连接以 HostKeyRejected 失败，不进入认证。
    #[test]
    fn sftp_control_connection_fails_after_identity_rejection() {
        use crate::core::host_identity::{HostIdentityService, PresentedHostKey};
        use std::time::Instant;
        use tauri::test::mock_app;

        let app = mock_app();
        let identity = HostIdentityService::new();
        let service = SftpService::with_verifying_connector(
            |host, _role, verifier| {
                verifier(&PresentedHostKey {
                    host: host.host.clone(),
                    port: host.port,
                    algorithm: "ssh-ed25519".to_string(),
                    fingerprint: "SHA256:sftp-deny".to_string(),
                    blob: b"blob".to_vec(),
                })?;
                Ok(empty_sftp())
            },
            TransferClock::system(),
            TRANSFER_IDLE_TIMEOUT,
        );
        service.register_session_with_verifier(
            "session-deny".to_string(),
            make_host(),
            identity.verifier(app.handle().clone(), "session-deny".to_string()),
        );

        let deadline = Instant::now() + Duration::from_secs(2);
        while identity.pending_challenge("session-deny").is_none() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        let challenge = identity.pending_challenge("session-deny").unwrap();
        identity.reject(&challenge.challenge_id).unwrap();

        // 拒绝后目录操作以 HostKeyRejected 语义失败（连接交付失败错误）
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if let Err(error) = service.list_dir("session-deny", "/") {
                assert!(
                    error.to_string().contains("主机身份"),
                    "拒绝后 SFTP 失败应包含主机身份语义，实际: {error}"
                );
                break;
            }
            assert!(Instant::now() < deadline, "拒绝后 SFTP 控制连接应失败");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// capability 回归（issue #35）：传输连接与控制连接一样经过统一主机身份校验，
    /// 不得绕过。第一次下载的传输连接以 Session 临时信任放行；服务端更换 key 后，
    /// 第二次下载新建的传输连接必须阻塞在校验门后产生新 challenge，拒绝即失败且
    /// 不进入认证。
    #[tokio::test(flavor = "multi_thread")]
    async fn transfer_connection_waits_for_host_identity_decision() {
        use crate::core::host_identity::{HostIdentityService, PresentedHostKey};
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::time::Instant;
        use tauri::test::mock_app;

        let app = mock_app();
        let identity = HostIdentityService::new();
        let key_seq = Arc::new(AtomicUsize::new(0));
        let control_attempts = Arc::new(AtomicUsize::new(0));
        let transfer_attempts = Arc::new(AtomicUsize::new(0));
        let key_for_connector = key_seq.clone();
        let control_for_connector = control_attempts.clone();
        let transfer_for_connector = transfer_attempts.clone();
        let service = SftpService::with_verifying_connector(
            move |host, role, verifier| {
                // 建连尝试先计数，再进入统一校验（握手后、认证前）；任何角色都不得绕过
                let transfer_attempt = match role {
                    SftpRole::Control => {
                        control_for_connector.fetch_add(1, Ordering::SeqCst);
                        None
                    }
                    SftpRole::Transfer => {
                        Some(transfer_for_connector.fetch_add(1, Ordering::SeqCst) + 1)
                    }
                };
                verifier(&PresentedHostKey {
                    host: host.host.clone(),
                    port: host.port,
                    algorithm: "ssh-ed25519".to_string(),
                    fingerprint: format!("SHA256:key-{}", key_for_connector.load(Ordering::SeqCst)),
                    blob: b"blob".to_vec(),
                })?;
                match role {
                    SftpRole::Control => Ok(memory_sftp(vec![7u8; 8])),
                    SftpRole::Transfer => {
                        // 第一次传输连接读完即失败（通道错误）→ 不健康归还并淘汰，
                        // 强制第二次下载新建传输连接（证明建连路径同样被校验门拦截）
                        if transfer_attempt == Some(1) {
                            Ok(channel_failing_transfer_sftp())
                        } else {
                            Ok(memory_sftp(vec![7u8; 8]))
                        }
                    }
                }
            },
            TransferClock::system(),
            TRANSFER_IDLE_TIMEOUT,
        );
        service.register_session_with_verifier(
            "session-transfer".to_string(),
            make_host(),
            identity.verifier(app.handle().clone(), "session-transfer".to_string()),
        );

        // 控制连接 eager 到达校验门：challenge（key-0）→ 仅本次接受
        let deadline = Instant::now() + Duration::from_secs(2);
        while identity.pending_challenge("session-transfer").is_none() && Instant::now() < deadline
        {
            std::thread::sleep(Duration::from_millis(10));
        }
        let challenge = identity
            .pending_challenge("session-transfer")
            .expect("控制连接产生 challenge");
        identity.accept(&challenge.challenge_id).unwrap();

        // 第一次下载：传输连接呈现 key-0（Session 临时信任）→ 放行进入传输，
        // 通道失败后连接被淘汰，任务以结构化通道错误失败
        let local_path =
            std::env::temp_dir().join(format!("titan-identity-transfer-{}.bin", Uuid::new_v4()));
        let first = service
            .enqueue_download(
                "session-transfer".to_string(),
                "/remote/file.bin".to_string(),
                local_path.to_string_lossy().to_string(),
                ConflictStrategy::Reject,
                app.handle().clone(),
            )
            .expect("下载应入队");
        assert_eq!(
            wait_for_terminal(&service, &first.task_id),
            SftpTaskStatus::Failed
        );
        assert_eq!(transfer_attempts.load(Ordering::SeqCst), 1);
        let _ = std::fs::remove_file(&local_path);

        // 服务端更换 key（key-1）：第二次下载必须新建传输连接（旧连接已淘汰），
        // 新连接呈现 key-1 → 新 challenge，阻塞在校验门后，绝不借旧决定放行
        key_seq.store(1, Ordering::SeqCst);
        let local_path2 =
            std::env::temp_dir().join(format!("titan-identity-transfer2-{}.bin", Uuid::new_v4()));
        let second = service
            .enqueue_download(
                "session-transfer".to_string(),
                "/remote/file.bin".to_string(),
                local_path2.to_string_lossy().to_string(),
                ConflictStrategy::Reject,
                app.handle().clone(),
            )
            .expect("下载应入队");

        let deadline = Instant::now() + Duration::from_secs(2);
        while identity.pending_challenge("session-transfer").is_none() && Instant::now() < deadline
        {
            std::thread::sleep(Duration::from_millis(10));
        }
        let challenge2 = identity
            .pending_challenge("session-transfer")
            .expect("新 key 的传输连接产生 challenge");
        assert_eq!(challenge2.fingerprint, "SHA256:key-1");
        assert_eq!(
            transfer_attempts.load(Ordering::SeqCst),
            2,
            "第二次下载新建传输连接且经过校验器"
        );
        assert_eq!(
            identity.waiting_connections(&challenge2.challenge_id),
            1,
            "传输连接阻塞在校验门后"
        );

        // 拒绝：传输以 HostKeyRejected 失败，不进入认证
        identity.reject(&challenge2.challenge_id).unwrap();
        assert_eq!(
            wait_for_terminal(&service, &second.task_id),
            SftpTaskStatus::Failed,
            "拒绝后第二次下载必须失败"
        );
        let error = service
            .tasks
            .lock()
            .unwrap()
            .get(&second.task_id)
            .and_then(|task| task.error.clone())
            .expect("失败任务应携带结构化错误");
        assert_eq!(error.code, "HostKeyRejected", "拒绝不得让传输进入认证");
        let _ = std::fs::remove_file(&local_path2);
    }

    /// capability 回归（issue #35）：失效控制连接自动重连时同样经过统一主机身份
    /// 校验，不得绕过共享校验门。仅本次接受的 Session 临时信任直接放行重连，
    /// 但校验器必须被再次调用。
    #[test]
    fn sftp_control_reconnect_still_goes_through_host_identity_verifier() {
        use crate::core::host_identity::{HostIdentityService, HostKeyVerifier, PresentedHostKey};
        use std::sync::atomic::AtomicUsize;
        use std::time::Instant;
        use tauri::test::mock_app;

        let app = mock_app();
        let identity = HostIdentityService::new();
        let attempts = Arc::new(AtomicUsize::new(0));
        let attempts_for_connector = attempts.clone();
        let verifier_calls = Arc::new(AtomicUsize::new(0));
        let service = SftpService::with_verifying_connector(
            move |host, role, verifier| {
                assert_eq!(role, SftpRole::Control, "本测试只驱动控制连接");
                verifier(&PresentedHostKey {
                    host: host.host.clone(),
                    port: host.port,
                    algorithm: "ssh-ed25519".to_string(),
                    fingerprint: "SHA256:sftp-reconnect".to_string(),
                    blob: b"blob".to_vec(),
                })?;
                if attempts_for_connector.fetch_add(1, Ordering::SeqCst) == 0 {
                    Ok(failing_channel_sftp())
                } else {
                    Ok(empty_sftp())
                }
            },
            TransferClock::system(),
            TRANSFER_IDLE_TIMEOUT,
        );
        // 计数包装器观察每一次校验器调用；重连路径不得绕过
        let inner = identity.verifier(app.handle().clone(), "session-reconnect".to_string());
        let calls_for_wrapper = verifier_calls.clone();
        let wrapped: HostKeyVerifier = Arc::new(move |presented| {
            calls_for_wrapper.fetch_add(1, Ordering::SeqCst);
            inner(presented)
        });
        service.register_session_with_verifier(
            "session-reconnect".to_string(),
            make_host(),
            wrapped,
        );

        // 首次连接：challenge → 仅本次接受放行
        let deadline = Instant::now() + Duration::from_secs(2);
        while identity.pending_challenge("session-reconnect").is_none() && Instant::now() < deadline
        {
            std::thread::sleep(Duration::from_millis(10));
        }
        let challenge = identity
            .pending_challenge("session-reconnect")
            .expect("SFTP 连接产生主机身份 challenge");
        identity.accept(&challenge.challenge_id).unwrap();

        // 首次 Ready 连接失效 → 自动重连一次；重连同样经过统一校验器
        assert!(
            service.list_dir("session-reconnect", "/").is_ok(),
            "重连后的目录操作应成功"
        );
        assert_eq!(attempts.load(Ordering::SeqCst), 2, "失效连接应重连一次");
        assert_eq!(
            verifier_calls.load(Ordering::SeqCst),
            2,
            "重连同样经过统一主机身份校验，不得绕过"
        );
    }

    /// 后台 eager 失败只交付一次，下一次操作必须触发重连。
    #[test]
    fn eager_failure_is_reported_once_then_next_operation_retries() {
        let attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let attempts_for_connector = attempts.clone();
        let service = SftpService::with_connector(move |_, _| {
            if attempts_for_connector.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                Err(AppError::SshConnectionError(
                    "first failure".to_string().into(),
                ))
            } else {
                Ok(empty_sftp())
            }
        });

        service.register_session("session-1".to_string(), make_host());
        let first = service.list_dir("session-1", "/");
        let second = service.list_dir("session-1", "/");

        assert!(first.unwrap_err().to_string().contains("first failure"));
        assert!(second.is_ok());
        assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    // ─── 失效控制连接自动重连一次 contract ──────────────────────────────────

    /// 目录操作发现 Ready 连接失效后淘汰旧连接并自动重连一次；重连成功返回结果。
    #[test]
    fn list_dir_evicts_stale_ready_connection_and_reconnects_once() {
        let attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let attempts_for_connector = attempts.clone();
        let service = SftpService::with_connector(move |_, _| {
            if attempts_for_connector.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                Ok(failing_channel_sftp())
            } else {
                Ok(empty_sftp())
            }
        });
        service.register_session("session-1".to_string(), make_host());

        assert!(
            service.list_dir("session-1", "/").is_ok(),
            "重连后的目录操作应成功"
        );
        assert_eq!(
            attempts.load(std::sync::atomic::Ordering::SeqCst),
            2,
            "失效连接应恰好淘汰并重连一次"
        );
    }

    /// 重连后的目录操作再次失败时返回第二次的结构化错误，且不进行无限重试。
    #[test]
    fn list_dir_second_failure_returns_structured_error_without_retry_loop() {
        let attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let attempts_for_connector = attempts.clone();
        let service = SftpService::with_connector(move |_, _| {
            attempts_for_connector.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(failing_channel_sftp())
        });
        service.register_session("session-1".to_string(), make_host());

        let error = service.list_dir("session-1", "/").unwrap_err();
        assert!(
            matches!(&error, AppError::SftpChannelError(message) if message.to_string().contains("connection lost")),
            "第二次失败应保留结构化通道错误，实际: {:?}",
            error
        );
        assert_eq!(
            attempts.load(std::sync::atomic::Ordering::SeqCst),
            2,
            "第二次失败后不得无限重连"
        );
    }

    /// 域错误（路径不存在）不触发淘汰重连，避免为健康连接支付重连成本。
    #[test]
    fn list_dir_domain_error_does_not_evict_connection() {
        let attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let attempts_for_connector = attempts.clone();
        let service = SftpService::with_connector(move |_, _| {
            attempts_for_connector.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(path_not_found_sftp())
        });
        service.register_session("session-1".to_string(), make_host());

        let error = service.list_dir("session-1", "/ghost").unwrap_err();
        assert!(
            matches!(error, AppError::SftpPathNotFound(path) if path.to_string() == "/ghost"),
            "域错误应原样返回"
        );
        assert_eq!(
            attempts.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "域错误不得触发淘汰重连"
        );
    }

    /// invalidate_if_ready 只淘汰本次操作实际使用的连接；其他操作刚重建的
    /// 健康新连接不得被迟到旧操作的淘汰请求误杀（同一失效连接只允许一次重建）。
    #[test]
    fn invalidate_if_ready_only_evicts_the_given_transport() {
        let connector: SftpConnector = Arc::new(|_, _, _| Ok(empty_sftp()));
        let connection = Arc::new(SftpConnection::new(
            make_host(),
            test_allow_all_verifier(),
            connector,
            SftpRole::Control,
        ));
        connection.connect_eager();

        let first = connection.get().unwrap();
        // 旧操作使用的连接被淘汰后，其他操作已重建新连接
        connection.invalidate_if_ready(&first);
        let second = connection.get().unwrap();
        assert!(!Arc::ptr_eq(&first, &second), "淘汰后 get 应重建新连接");

        // 迟到的旧操作失败只能淘汰它自己使用过的连接，不得误杀新连接
        connection.invalidate_if_ready(&first);
        let current = connection.get().unwrap();
        assert!(
            Arc::ptr_eq(&current, &second),
            "新连接不得被旧操作的淘汰请求误杀"
        );
    }

    /// 元数据操作（file_size）发现失效控制连接时淘汰并重连一次；
    /// 传输随后通过独立传输连接正常完成，不得复用或重建控制连接。
    #[tokio::test(flavor = "multi_thread")]
    async fn enqueue_download_evicts_stale_connection_for_file_size() {
        use std::sync::atomic::AtomicUsize;
        use tauri::test::mock_app;

        let app = mock_app();
        let control_attempts = Arc::new(AtomicUsize::new(0));
        let transfer_attempts = Arc::new(AtomicUsize::new(0));
        let control_attempts_for_connector = control_attempts.clone();
        let transfer_attempts_for_connector = transfer_attempts.clone();
        let service = SftpService::with_connector(move |_, role| match role {
            SftpRole::Control => {
                if control_attempts_for_connector.fetch_add(1, Ordering::SeqCst) == 0 {
                    Ok(failing_channel_sftp())
                } else {
                    Ok(memory_sftp(vec![1u8, 2, 3]))
                }
            }
            SftpRole::Transfer => {
                transfer_attempts_for_connector.fetch_add(1, Ordering::SeqCst);
                Ok(memory_sftp(vec![1u8, 2, 3]))
            }
        });
        service.register_session("session-1".to_string(), make_host());

        let local_path =
            std::env::temp_dir().join(format!("titan-reconnect-{}.bin", Uuid::new_v4()));
        let task = service
            .enqueue_download(
                "session-1".to_string(),
                "/remote/file.bin".to_string(),
                local_path.to_string_lossy().to_string(),
                ConflictStrategy::Reject,
                app.handle().clone(),
            )
            .expect("重连后的元数据操作应成功入队");

        assert_eq!(
            control_attempts.load(Ordering::SeqCst),
            2,
            "file_size 失效控制连接应恰好淘汰并重连一次"
        );
        assert_eq!(
            wait_for_terminal(&service, &task.task_id),
            SftpTaskStatus::Done,
            "重连后的传输应正常完成"
        );
        assert_eq!(
            transfer_attempts.load(Ordering::SeqCst),
            1,
            "传输应通过独立传输连接完成，不得复用控制连接"
        );
        let _ = std::fs::remove_file(&local_path);
    }

    /// file_size 重连后仍失败：enqueue_download 直接返回结构化错误，不无限重试。
    #[test]
    fn enqueue_download_second_file_size_failure_returns_structured_error() {
        use tauri::test::mock_app;

        let app = mock_app();
        let attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let attempts_for_connector = attempts.clone();
        let service = SftpService::with_connector(move |_, _| {
            attempts_for_connector.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(failing_channel_sftp())
        });
        service.register_session("session-1".to_string(), make_host());

        let local_path =
            std::env::temp_dir().join(format!("titan-reconnect-fail-{}.bin", Uuid::new_v4()));
        let error = service
            .enqueue_download(
                "session-1".to_string(),
                "/remote/file.bin".to_string(),
                local_path.to_string_lossy().to_string(),
                ConflictStrategy::Reject,
                app.handle().clone(),
            )
            .unwrap_err();
        assert!(
            matches!(&error, AppError::SftpChannelError(message) if message.to_string().contains("connection lost")),
            "第二次元数据失败应保留结构化通道错误，实际: {:?}",
            error
        );
        assert_eq!(
            attempts.load(std::sync::atomic::Ordering::SeqCst),
            2,
            "第二次失败后不得无限重连"
        );
    }

    /// 同一 Session 已有 Pending/Running 下载占用相同最终目标时，
    /// 后加入任务被拒绝并返回结构化 SftpTargetBusy 错误；原任务不受影响。
    #[tokio::test(flavor = "multi_thread")]
    async fn enqueue_download_duplicate_active_target_is_rejected() {
        use tauri::test::mock_app;

        let app = mock_app();
        let transfer_started = Arc::new(Barrier::new(2));
        let transfer_release = Arc::new(Barrier::new(2));
        let started_for_connector = transfer_started.clone();
        let release_for_connector = transfer_release.clone();
        let service = SftpService::with_connector(move |_, role| match role {
            SftpRole::Control => Ok(memory_sftp(vec![7u8; 8])),
            SftpRole::Transfer => Ok(blocking_read_sftp(
                started_for_connector.clone(),
                release_for_connector.clone(),
            )),
        });
        service.register_session("session-1".to_string(), make_host());

        let local_path = std::env::temp_dir().join(format!("titan-dup-{}.bin", Uuid::new_v4()));
        let first = service
            .enqueue_download(
                "session-1".to_string(),
                "/remote/file.bin".to_string(),
                local_path.to_string_lossy().to_string(),
                ConflictStrategy::Reject,
                app.handle().clone(),
            )
            .expect("首个下载应正常入队");
        transfer_started.wait(); // 首个任务进入阻塞读取（Pending 或 Running）

        let error = service
            .enqueue_download(
                "session-1".to_string(),
                "/remote/file.bin".to_string(),
                local_path.to_string_lossy().to_string(),
                ConflictStrategy::Reject,
                app.handle().clone(),
            )
            .expect_err("相同最终目标的第二个下载应被拒绝");
        let expected_target = local_path.to_string_lossy().to_string();
        assert!(
            matches!(&error, AppError::SftpTargetBusy(path) if path.to_string() == expected_target),
            "重复目标应返回结构化 SftpTargetBusy 错误，实际: {:?}",
            error
        );

        transfer_release.wait();
        assert_eq!(
            wait_for_terminal(&service, &first.task_id),
            SftpTaskStatus::Done,
            "首个任务不受拒绝影响，应正常完成"
        );
        let _ = std::fs::remove_file(&local_path);
    }

    /// 最终目标不含文件名（如以 .. 结尾、父目录存在）时入队被拒绝：
    /// 临时文件必须以目标文件名为基、与最终目标同目录，无法满足时宁可拒绝也不降级。
    #[test]
    fn enqueue_download_rejects_target_without_file_name() {
        use tauri::test::mock_app;

        let app = mock_app();
        let service = make_service();
        service.register_session("session-1".to_string(), make_host());

        let local_dir = std::env::temp_dir().join(format!("titan-nofile-{}", Uuid::new_v4()));
        std::fs::create_dir(&local_dir).unwrap();
        let local_path = local_dir.join(".."); // 父目录存在，但路径以 .. 结尾、无文件名

        let error = service
            .enqueue_download(
                "session-1".to_string(),
                "/remote/file.bin".to_string(),
                local_path.to_string_lossy().to_string(),
                ConflictStrategy::Reject,
                app.handle().clone(),
            )
            .unwrap_err();
        assert!(
            matches!(&error, AppError::SftpTransferError(message) if message.to_string().contains("本地路径无效")),
            "不含文件名的目标应被拒绝，实际: {:?}",
            error
        );
        let _ = std::fs::remove_dir(&local_dir);
    }

    /// 一个 Session 的慢目录读取不得持有其他 Session 所需的 registry 锁。
    #[test]
    fn slow_directory_read_does_not_block_another_session() {
        let started = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let connector_started = started.clone();
        let connector_release = release.clone();
        let service = SftpService::with_connector(move |host, _| {
            if host.id == "slow" {
                Ok(blocking_sftp(
                    connector_started.clone(),
                    connector_release.clone(),
                ))
            } else {
                Ok(empty_sftp())
            }
        });
        let mut slow_host = make_host();
        slow_host.id = "slow".to_string();
        let mut fast_host = make_host();
        fast_host.id = "fast".to_string();
        service.register_session("slow-session".to_string(), slow_host);
        service.register_session("fast-session".to_string(), fast_host);

        let slow_service = service.clone();
        let slow = std::thread::spawn(move || slow_service.list_dir("slow-session", "/"));
        started.wait();

        let fast_service = service.clone();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = done_tx.send(fast_service.list_dir("fast-session", "/"));
        });
        let fast_result = done_rx
            .recv_timeout(Duration::from_millis(200))
            .expect("其他 Session 不应被慢目录读取阻塞");
        release.wait();

        assert!(fast_result.is_ok());
        assert!(slow.join().unwrap().is_ok());
    }

    /// Session 关闭后，阻塞建连的迟到结果必须被释放且不得重新注册。
    #[test]
    fn closed_session_discards_late_sftp_connection() {
        use tauri::test::mock_app;

        let started = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let started_for_connector = started.clone();
        let release_for_connector = release.clone();
        let (dropped_tx, dropped_rx) = std::sync::mpsc::channel();
        let service = SftpService::with_connector(move |_, _| {
            started_for_connector.wait();
            release_for_connector.wait();
            Ok(drop_signal_sftp(dropped_tx.clone()))
        });
        service.register_session("session-1".to_string(), make_host());
        started.wait();

        let app = mock_app();
        service.cleanup_session("session-1", &app.handle().clone());
        release.wait();

        dropped_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("迟到 transport 应立即释放");
        assert!(!service.has_session("session-1"));
    }

    // ─── 控制/传输连接分离 contract ──────────────────────────────────────

    /// 传输连接按需建立：Session 注册只建立控制连接，首次传输才建立独立传输连接。
    #[tokio::test(flavor = "multi_thread")]
    async fn transfer_connection_is_lazy_and_independent_from_control() {
        use crate::core::ssh_transport::test_support::{in_memory_sftp, in_memory_sftp_transport};
        use std::sync::atomic::AtomicUsize;
        use tauri::test::mock_app;

        let app = mock_app();
        let control_connects = Arc::new(AtomicUsize::new(0));
        let transfer_connects = Arc::new(AtomicUsize::new(0));
        let control_connects_for_connector = control_connects.clone();
        let transfer_connects_for_connector = transfer_connects.clone();
        let fs = in_memory_sftp(&[]);
        let fs_for_connector = fs.clone();
        let service = SftpService::with_connector(move |_, role| match role {
            SftpRole::Control => {
                control_connects_for_connector.fetch_add(1, Ordering::SeqCst);
                Ok(empty_sftp())
            }
            SftpRole::Transfer => {
                transfer_connects_for_connector.fetch_add(1, Ordering::SeqCst);
                Ok(in_memory_sftp_transport(&fs_for_connector))
            }
        });
        service.register_session("session-1".to_string(), make_host());

        // 控制连接 eager 建连后即可目录操作
        assert!(service.list_dir("session-1", "/").is_ok());
        assert_eq!(
            control_connects.load(Ordering::SeqCst),
            1,
            "注册后只建立控制连接"
        );
        assert_eq!(
            transfer_connects.load(Ordering::SeqCst),
            0,
            "无传输时不得预建传输连接"
        );

        // 首次传输才按需建立传输连接
        let local_path = std::env::temp_dir().join(format!("titan-lazy-tx-{}.bin", Uuid::new_v4()));
        std::fs::write(&local_path, b"hello").unwrap();
        let task = service
            .enqueue_upload(
                "session-1".to_string(),
                local_path.to_string_lossy().to_string(),
                "/tmp".to_string(),
                ConflictStrategy::Reject,
                app.handle().clone(),
            )
            .unwrap();
        assert_eq!(
            wait_for_terminal(&service, &task.task_id),
            SftpTaskStatus::Done
        );
        assert_eq!(
            transfer_connects.load(Ordering::SeqCst),
            1,
            "首次传输应建立一条独立传输连接"
        );
        assert_eq!(
            control_connects.load(Ordering::SeqCst),
            1,
            "传输不得重建控制连接"
        );
        let _ = std::fs::remove_file(&local_path);
    }

    /// 传输连接失效（通道错误）后淘汰传输连接，下一次传输自动重建；
    /// 本次任务保留结构化错误，不自动重跑。
    #[tokio::test(flavor = "multi_thread")]
    async fn failed_transfer_invalidates_transfer_connection_for_next_task() {
        use crate::core::ssh_transport::test_support::{in_memory_sftp, in_memory_sftp_transport};
        use std::sync::atomic::AtomicUsize;
        use tauri::test::mock_app;

        let app = mock_app();
        let transfer_attempts = Arc::new(AtomicUsize::new(0));
        let transfer_attempts_for_connector = transfer_attempts.clone();
        let fs = in_memory_sftp(&[]);
        let fs_for_connector = fs.clone();
        let service = SftpService::with_connector(move |_, role| match role {
            SftpRole::Control => Ok(empty_sftp()),
            SftpRole::Transfer => {
                if transfer_attempts_for_connector.fetch_add(1, Ordering::SeqCst) == 0 {
                    Ok(channel_failing_transfer_sftp())
                } else {
                    Ok(in_memory_sftp_transport(&fs_for_connector))
                }
            }
        });
        service.register_session("session-1".to_string(), make_host());

        // 第一次上传：传输连接建立即失败，任务保留结构化通道错误
        let local_path =
            std::env::temp_dir().join(format!("titan-tx-reconnect-{}.bin", Uuid::new_v4()));
        std::fs::write(&local_path, b"hello").unwrap();
        let first = service
            .enqueue_upload(
                "session-1".to_string(),
                local_path.to_string_lossy().to_string(),
                "/tmp".to_string(),
                ConflictStrategy::Reject,
                app.handle().clone(),
            )
            .unwrap();
        assert_eq!(
            wait_for_terminal(&service, &first.task_id),
            SftpTaskStatus::Failed,
            "失效传输连接上的任务应失败"
        );
        let first_task = service
            .tasks
            .lock()
            .unwrap()
            .get(&first.task_id)
            .unwrap()
            .clone();
        assert_eq!(
            first_task.error.as_ref().map(|e| e.code.as_str()),
            Some("SftpChannelError"),
            "失效传输连接应保留结构化通道错误"
        );

        // 第二次上传：失效传输连接已淘汰，重建后正常完成
        let second = service
            .enqueue_upload(
                "session-1".to_string(),
                local_path.to_string_lossy().to_string(),
                "/tmp".to_string(),
                ConflictStrategy::Reject,
                app.handle().clone(),
            )
            .unwrap();
        assert_eq!(
            wait_for_terminal(&service, &second.task_id),
            SftpTaskStatus::Done,
            "淘汰后重建的传输连接应完成任务"
        );
        assert_eq!(
            transfer_attempts.load(Ordering::SeqCst),
            2,
            "失效传输连接应恰好淘汰并重建一次"
        );
        let _ = std::fs::remove_file(&local_path);
    }

    /// 传输在独立传输连接上阻塞时，控制连接上的目录列举与元数据操作仍能完成。
    #[tokio::test(flavor = "multi_thread")]
    async fn control_operations_complete_while_transfer_is_blocked() {
        use tauri::test::mock_app;

        let app = mock_app();
        let transfer_started = Arc::new(Barrier::new(2));
        let transfer_release = Arc::new(Barrier::new(2));
        let transfer_connects = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let started_for_connector = transfer_started.clone();
        let release_for_connector = transfer_release.clone();
        let connects_for_connector = transfer_connects.clone();
        let service = SftpService::with_connector(move |_, role| match role {
            SftpRole::Control => Ok(memory_sftp(vec![7u8; 4096])),
            SftpRole::Transfer => {
                // 第一条传输连接在 barrier 之间阻塞；第二条及后续连接立即完成：
                // 传输连接池为每个并发任务分配独立连接，不复用同一 adapter 的阻塞状态
                if connects_for_connector.fetch_add(1, Ordering::SeqCst) == 0 {
                    Ok(blocking_read_sftp(
                        started_for_connector.clone(),
                        release_for_connector.clone(),
                    ))
                } else {
                    Ok(memory_sftp(vec![7u8; 8]))
                }
            }
        });
        service.register_session("session-1".to_string(), make_host());

        let local_path = std::env::temp_dir().join(format!("titan-blocked-{}.bin", Uuid::new_v4()));
        let task = service
            .enqueue_download(
                "session-1".to_string(),
                "/remote/file.bin".to_string(),
                local_path.to_string_lossy().to_string(),
                ConflictStrategy::Reject,
                app.handle().clone(),
            )
            .unwrap();
        transfer_started.wait(); // 传输已进入阻塞读取

        // 传输阻塞期间目录列举必须完成，不等传输结束
        let list_service = service.clone();
        let (list_tx, list_rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = list_tx.send(list_service.list_dir("session-1", "/"));
        });
        let list_result = list_rx
            .recv_timeout(Duration::from_millis(500))
            .expect("传输阻塞期间目录列举应完成");
        assert!(
            list_result.is_ok(),
            "控制连接上的目录列举应成功，实际: {:?}",
            list_result
        );

        // enqueue_download 同步完成控制连接上的 file_size 后才返回：
        // 入队成功即证明元数据操作在传输阻塞期间即时完成
        let local_second =
            std::env::temp_dir().join(format!("titan-blocked-2-{}.bin", Uuid::new_v4()));
        let second = service
            .enqueue_download(
                "session-1".to_string(),
                "/remote/file.bin".to_string(),
                local_second.to_string_lossy().to_string(),
                ConflictStrategy::Reject,
                app.handle().clone(),
            )
            .expect("传输运行期间 file_size 元数据操作应在控制连接上完成入队");

        transfer_release.wait();
        assert_eq!(
            wait_for_terminal(&service, &task.task_id),
            SftpTaskStatus::Done
        );
        assert_eq!(
            wait_for_terminal(&service, &second.task_id),
            SftpTaskStatus::Done
        );
        let _ = std::fs::remove_file(&local_path);
        let _ = std::fs::remove_file(&local_second);
    }

    /// Session 关闭同时关闭控制与传输 capability；阻塞建连的迟到传输结果被丢弃，
    /// 迟到 worker 的终态迁移被拒绝，不残留任务。
    #[test]
    fn closed_session_discards_late_transfer_connection() {
        use tauri::test::mock_app;

        let started = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let started_for_connector = started.clone();
        let release_for_connector = release.clone();
        let (dropped_tx, dropped_rx) = std::sync::mpsc::channel();
        let service = SftpService::with_connector(move |_, role| match role {
            SftpRole::Control => Ok(empty_sftp()),
            SftpRole::Transfer => {
                started_for_connector.wait();
                release_for_connector.wait();
                Ok(drop_signal_sftp(dropped_tx.clone()))
            }
        });
        service.register_session("session-1".to_string(), make_host());

        let app = mock_app();
        // 启动一次上传：worker 开始按需建立传输连接（阻塞在建连中）
        let local_path =
            std::env::temp_dir().join(format!("titan-late-transfer-{}.bin", Uuid::new_v4()));
        std::fs::write(&local_path, b"hello").unwrap();
        service
            .enqueue_upload(
                "session-1".to_string(),
                local_path.to_string_lossy().to_string(),
                "/tmp".to_string(),
                ConflictStrategy::Reject,
                app.handle().clone(),
            )
            .unwrap();
        started.wait(); // 传输建连进行中

        service.cleanup_session("session-1", &app.handle().clone());
        release.wait();

        dropped_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("迟到传输连接应立即释放");
        assert!(!service.has_session("session-1"));
        wait_until(
            || service.task_snapshot("session-1").is_empty().then_some(()),
            Duration::from_secs(1),
        )
        .expect("迟到 worker 应发布 Cancelled 后清理任务");
        let _ = std::fs::remove_file(&local_path);
    }

    // ─── 五路传输连接池 contract ─────────────────────────────────────────

    /// 单个 Session 同时最多五个 Running 传输：五路各持独立传输连接，
    /// 第六个保持 Pending；释放一个名额后第六个复用连接启动。
    #[tokio::test(flavor = "multi_thread")]
    async fn five_transfers_run_concurrently_and_sixth_waits_for_freed_slot() {
        use crate::core::ssh_transport::test_support::{Gate, counted_sftp, gated_in_memory_sftp};
        use tauri::test::mock_app;

        let app = mock_app();
        let live = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let connects = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let gates: Arc<std::sync::Mutex<Vec<Arc<Gate>>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));
        let live_for_connector = live.clone();
        let connects_for_connector = connects.clone();
        let gates_for_connector = gates.clone();
        let service = SftpService::with_connector(move |_, role| match role {
            SftpRole::Control => Ok(memory_sftp(Vec::new())),
            SftpRole::Transfer => {
                connects_for_connector.fetch_add(1, Ordering::SeqCst);
                let gate = Gate::new();
                gates_for_connector.lock().unwrap().push(gate.clone());
                Ok(counted_sftp(
                    gated_in_memory_sftp(&[], gate, false),
                    live_for_connector.clone(),
                ))
            }
        });
        service.register_session("session-1".to_string(), make_host());

        // 六个本地文件逐个入队
        let mut local_paths = Vec::new();
        for index in 0..6 {
            let path =
                std::env::temp_dir().join(format!("titan-pool-{}-{}.bin", Uuid::new_v4(), index));
            std::fs::write(&path, b"data").unwrap();
            local_paths.push(path);
        }
        let tasks: Vec<TransferTask> = local_paths
            .iter()
            .map(|path| {
                service
                    .enqueue_upload(
                        "session-1".to_string(),
                        path.to_string_lossy().to_string(),
                        "/tmp".to_string(),
                        ConflictStrategy::Reject,
                        app.handle().clone(),
                    )
                    .expect("上传应正常入队")
            })
            .collect();

        // 五路 Running 后第六个必须保持 Pending：等待者是 registry 中唯一的 Pending 任务，
        // 不假设一定是最后入队者（worker 调度顺序与入队顺序无关）
        let sixth_id = wait_until(
            || {
                let snapshot = service.task_snapshot("session-1");
                let running = snapshot
                    .iter()
                    .filter(|task| task.status == SftpTaskStatus::Running)
                    .count();
                let pending: Vec<String> = snapshot
                    .iter()
                    .filter(|task| task.status == SftpTaskStatus::Pending)
                    .map(|task| task.task_id.clone())
                    .collect();
                (running == 5 && pending.len() == 1)
                    .then_some(pending.into_iter().next())
                    .flatten()
            },
            Duration::from_secs(5),
        )
        .expect("应有恰好五个 Running 传输与一个 Pending 任务");
        assert_eq!(
            live.load(Ordering::SeqCst),
            5,
            "五个运行任务各持独立传输连接"
        );
        assert_eq!(
            connects.load(Ordering::SeqCst),
            5,
            "传输连接应按需建立恰好五条"
        );

        // 释放一条连接 → 等待中的第六个任务复用该连接完成，其余四路仍在运行
        let gates_snapshot = gates.lock().unwrap().clone();
        assert_eq!(gates_snapshot.len(), 5, "五路运行应恰好持有五条连接");
        gates_snapshot[0].open();
        assert_eq!(
            wait_for_terminal(&service, &sixth_id),
            SftpTaskStatus::Done,
            "名额释放后等待任务应启动并完成"
        );
        assert_eq!(
            connects.load(Ordering::SeqCst),
            5,
            "等待任务应复用已释放连接，不得新建"
        );
        let remaining_running = service
            .task_snapshot("session-1")
            .iter()
            .filter(|task| task.status == SftpTaskStatus::Running)
            .count();
        assert_eq!(remaining_running, 4, "其余四路传输应仍在运行");

        // 放行全部 → 全部完成，空闲未超时连接不回收
        for gate in gates_snapshot.iter().skip(1) {
            gate.open();
        }
        for task in &tasks {
            assert_eq!(
                wait_for_terminal(&service, &task.task_id),
                SftpTaskStatus::Done
            );
        }
        assert_eq!(live.load(Ordering::SeqCst), 5, "空闲未超时不回收连接");
        for path in &local_paths {
            let _ = std::fs::remove_file(path);
        }
    }

    /// 名额释放后等待任务按入队顺序启动：Session 内 FIFO，一次释放只启动队首。
    #[tokio::test(flavor = "multi_thread")]
    async fn waiting_tasks_start_in_fifo_order_after_slot_release() {
        use crate::core::ssh_transport::test_support::{
            Gate, gated_in_memory_sftp, in_memory_sftp_transport,
        };
        use tauri::test::mock_app;

        let app = mock_app();
        let gates: Arc<std::sync::Mutex<Vec<Arc<Gate>>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));
        let running_events: Arc<std::sync::Mutex<Vec<String>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));
        let running_events_for_listener = running_events.clone();
        {
            use tauri::Listener;
            app.listen("sftp:task_status", move |event| {
                let payload: SftpTaskStatusEvent =
                    serde_json::from_str(event.payload()).expect("payload 应为结构化状态事件");
                if payload.status == SftpTaskStatus::Running {
                    running_events_for_listener
                        .lock()
                        .unwrap()
                        .push(payload.task_id);
                }
            });
        }
        let gates_for_connector = gates.clone();
        let service = SftpService::with_connector(move |_, role| match role {
            SftpRole::Control => Ok(memory_sftp(Vec::new())),
            SftpRole::Transfer => {
                let gate = Gate::new();
                gates_for_connector.lock().unwrap().push(gate.clone());
                Ok(in_memory_sftp_transport(&gated_in_memory_sftp(
                    &[],
                    gate,
                    false,
                )))
            }
        });
        service.register_session("session-1".to_string(), make_host());

        // 八个任务：前五个阻塞占满五路，后三个按 FIFO 等待
        let mut local_paths = Vec::new();
        for index in 0..8 {
            let path =
                std::env::temp_dir().join(format!("titan-fifo-{}-{}.bin", Uuid::new_v4(), index));
            std::fs::write(&path, b"data").unwrap();
            local_paths.push(path);
        }
        let tasks: Vec<TransferTask> = local_paths
            .iter()
            .map(|path| {
                service
                    .enqueue_upload(
                        "session-1".to_string(),
                        path.to_string_lossy().to_string(),
                        "/tmp".to_string(),
                        ConflictStrategy::Reject,
                        app.handle().clone(),
                    )
                    .expect("上传应正常入队")
            })
            .collect();
        // 等待者是 registry 中全部 Pending 任务：不假设一定是最后入队者。
        // 预期启动顺序 = 等待任务按入队先后排序。
        let parked_ids = wait_until(
            || {
                let snapshot = service.task_snapshot("session-1");
                let running = snapshot
                    .iter()
                    .filter(|task| task.status == SftpTaskStatus::Running)
                    .count();
                let pending: Vec<String> = snapshot
                    .iter()
                    .filter(|task| task.status == SftpTaskStatus::Pending)
                    .map(|task| task.task_id.clone())
                    .collect();
                (running == 5 && pending.len() == 3).then_some(pending)
            },
            Duration::from_secs(5),
        )
        .expect("五路 Running、三路 Pending");
        let mut expected_order: Vec<String> = parked_ids.clone();
        expected_order.sort_by_key(|task_id| {
            tasks
                .iter()
                .position(|task| &task.task_id == task_id)
                .expect("等待任务必须来自本测试的入队")
        });

        // 释放一条连接：三个等待任务级联复用该连接依次完成，
        // Running 事件按 Session 内 FIFO（入队先后）到达
        let gates_snapshot = gates.lock().unwrap().clone();
        gates_snapshot[0].open();
        for task_id in &parked_ids {
            assert_eq!(
                wait_for_terminal(&service, task_id),
                SftpTaskStatus::Done,
                "等待任务应级联完成"
            );
        }
        let observed_order: Vec<usize> = {
            let events = running_events.lock().unwrap();
            expected_order
                .iter()
                .map(|task_id| {
                    events
                        .iter()
                        .position(|event_id| event_id == task_id)
                        .expect("等待任务应有 Running 事件")
                })
                .collect()
        };
        assert!(
            observed_order[0] < observed_order[1] && observed_order[1] < observed_order[2],
            "等待任务应按 FIFO 顺序启动，实际 Running 顺序: {:?}",
            observed_order
        );

        for gate in gates_snapshot.iter().skip(1) {
            gate.open();
        }
        for task in &tasks {
            assert_eq!(
                wait_for_terminal(&service, &task.task_id),
                SftpTaskStatus::Done
            );
        }
        for path in &local_paths {
            let _ = std::fs::remove_file(path);
        }
    }

    /// 额外传输连接连续空闲 60 秒后回收为基础一条：确定性时间源推进，不等待真实 60 秒。
    #[tokio::test(flavor = "multi_thread")]
    async fn idle_extra_connections_recycled_after_sixty_seconds() {
        use crate::core::ssh_transport::test_support::{Gate, counted_sftp, gated_in_memory_sftp};
        use tauri::test::mock_app;

        let app = mock_app();
        let live = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let connects = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let gates: Arc<std::sync::Mutex<Vec<Arc<Gate>>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));
        let clock = Arc::new(TransferClock::manual());
        let live_for_connector = live.clone();
        let connects_for_connector = connects.clone();
        let gates_for_connector = gates.clone();
        let service = SftpService::with_connector_clock_timeout(
            move |_, role| match role {
                SftpRole::Control => Ok(memory_sftp(Vec::new())),
                SftpRole::Transfer => {
                    connects_for_connector.fetch_add(1, Ordering::SeqCst);
                    let gate = Gate::new();
                    gates_for_connector.lock().unwrap().push(gate.clone());
                    Ok(counted_sftp(
                        gated_in_memory_sftp(&[], gate, false),
                        live_for_connector.clone(),
                    ))
                }
            },
            (*clock).clone(),
            TRANSFER_IDLE_TIMEOUT,
        );
        service.register_session("session-1".to_string(), make_host());

        // 五路在放行门前同时阻塞 → 5 条连接共存（1 基础 + 4 额外）
        let mut local_paths = Vec::new();
        let mut tasks = Vec::new();
        for index in 0..5 {
            let path = std::env::temp_dir().join(format!(
                "titan-recycle-{}-{}.bin",
                Uuid::new_v4(),
                index
            ));
            std::fs::write(&path, b"data").unwrap();
            let task = service
                .enqueue_upload(
                    "session-1".to_string(),
                    path.to_string_lossy().to_string(),
                    "/tmp".to_string(),
                    ConflictStrategy::Reject,
                    app.handle().clone(),
                )
                .expect("上传应正常入队");
            tasks.push(task);
            local_paths.push(path);
        }
        wait_until(
            || {
                let snapshot = service.task_snapshot("session-1");
                let running = snapshot
                    .iter()
                    .filter(|task| task.status == SftpTaskStatus::Running)
                    .count();
                (running == 5).then_some(())
            },
            Duration::from_secs(5),
        )
        .expect("五路传输应同时运行");
        assert_eq!(live.load(Ordering::SeqCst), 5, "五路运行各持一条连接");
        assert_eq!(connects.load(Ordering::SeqCst), 5);
        let gates_snapshot = gates.lock().unwrap().clone();
        for gate in &gates_snapshot {
            gate.open();
        }
        for task in &tasks {
            assert_eq!(
                wait_for_terminal(&service, &task.task_id),
                SftpTaskStatus::Done
            );
        }
        assert_eq!(
            live.load(Ordering::SeqCst),
            5,
            "五路完成后五条连接应保持空闲"
        );

        // 推进 59 秒：未满 60 秒不得回收
        clock.advance(59_000);
        let path_before =
            std::env::temp_dir().join(format!("titan-recycle-before-{}.bin", Uuid::new_v4()));
        std::fs::write(&path_before, b"data").unwrap();
        let task_before = service
            .enqueue_upload(
                "session-1".to_string(),
                path_before.to_string_lossy().to_string(),
                "/tmp".to_string(),
                ConflictStrategy::Reject,
                app.handle().clone(),
            )
            .expect("上传应正常入队");
        assert_eq!(
            wait_for_terminal(&service, &task_before.task_id),
            SftpTaskStatus::Done
        );
        assert_eq!(
            live.load(Ordering::SeqCst),
            5,
            "空闲未满 60 秒不得回收额外连接"
        );

        // 补足 60 秒：下一次传输 checkout 时回收四条额外连接，只保留基础一条
        clock.advance(1_000);
        let path_after =
            std::env::temp_dir().join(format!("titan-recycle-after-{}.bin", Uuid::new_v4()));
        std::fs::write(&path_after, b"data").unwrap();
        let task_after = service
            .enqueue_upload(
                "session-1".to_string(),
                path_after.to_string_lossy().to_string(),
                "/tmp".to_string(),
                ConflictStrategy::Reject,
                app.handle().clone(),
            )
            .expect("上传应正常入队");
        assert_eq!(
            wait_for_terminal(&service, &task_after.task_id),
            SftpTaskStatus::Done
        );
        assert_eq!(
            live.load(Ordering::SeqCst),
            1,
            "额外连接空闲 60 秒后应回收为基础一条"
        );
        assert_eq!(
            connects.load(Ordering::SeqCst),
            5,
            "回收后复用基础连接，不得新建"
        );
        for path in local_paths {
            let _ = std::fs::remove_file(path);
        }
        let _ = std::fs::remove_file(&path_before);
        let _ = std::fs::remove_file(&path_after);
    }

    /// 传输连接建立失败只影响对应任务：结构化错误与合法迁移，池内其他任务不受影响。
    #[tokio::test(flavor = "multi_thread")]
    async fn transfer_connection_setup_failure_only_fails_affected_task() {
        use crate::core::ssh_transport::test_support::{
            Gate, gated_in_memory_sftp, in_memory_sftp, in_memory_sftp_transport,
        };
        use tauri::test::mock_app;

        let app = mock_app();
        let transfer_attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let gate = Gate::new();
        let gate_for_connector = gate.clone();
        let transfer_attempts_for_connector = transfer_attempts.clone();
        let fs = in_memory_sftp(&[]);
        let fs_for_connector = fs.clone();
        let service = SftpService::with_connector(move |_, role| match role {
            SftpRole::Control => Ok(memory_sftp(Vec::new())),
            SftpRole::Transfer => {
                match transfer_attempts_for_connector.fetch_add(1, Ordering::SeqCst) {
                    0 => Ok(in_memory_sftp_transport(&gated_in_memory_sftp(
                        &[],
                        gate_for_connector.clone(),
                        false,
                    ))),
                    1 => Err(AppError::SftpChannelError(
                        "transfer connect failed".to_string().into(),
                    )),
                    _ => Ok(in_memory_sftp_transport(&fs_for_connector)),
                }
            }
        });
        service.register_session("session-1".to_string(), make_host());

        // 第一个任务占住第一条连接
        let first_path =
            std::env::temp_dir().join(format!("titan-connect-fail-1-{}.bin", Uuid::new_v4()));
        std::fs::write(&first_path, b"data").unwrap();
        let first = service
            .enqueue_upload(
                "session-1".to_string(),
                first_path.to_string_lossy().to_string(),
                "/tmp".to_string(),
                ConflictStrategy::Reject,
                app.handle().clone(),
            )
            .expect("首个上传应正常入队");
        wait_until(
            || {
                let status = service
                    .tasks
                    .lock()
                    .unwrap()
                    .get(&first.task_id)
                    .map(|task| task.status.clone());
                (status == Some(SftpTaskStatus::Running)).then_some(())
            },
            Duration::from_secs(5),
        )
        .expect("首个任务应进入 Running");

        // 第二个任务建连失败：只影响该任务，保留结构化通道错误
        let second_path =
            std::env::temp_dir().join(format!("titan-connect-fail-2-{}.bin", Uuid::new_v4()));
        std::fs::write(&second_path, b"data").unwrap();
        let second = service
            .enqueue_upload(
                "session-1".to_string(),
                second_path.to_string_lossy().to_string(),
                "/tmp".to_string(),
                ConflictStrategy::Reject,
                app.handle().clone(),
            )
            .expect("入队本身应成功，失败发生在传输阶段");
        assert_eq!(
            wait_for_terminal(&service, &second.task_id),
            SftpTaskStatus::Failed,
            "建连失败的任务应进入 Failed"
        );
        let second_task = service
            .tasks
            .lock()
            .unwrap()
            .get(&second.task_id)
            .unwrap()
            .clone();
        assert_eq!(
            second_task.error.as_ref().map(|error| error.code.as_str()),
            Some("SftpChannelError"),
            "建连失败应保留结构化通道错误"
        );

        // 第三个任务在第一条连接仍被占用时重建连接并正常完成：池未被失败污染。
        // 先于放行第一个任务入队并等待完成，保证第三个任务只能通过新建连接完成，
        // 建连次数断言确定。
        let third_path =
            std::env::temp_dir().join(format!("titan-connect-fail-3-{}.bin", Uuid::new_v4()));
        std::fs::write(&third_path, b"data").unwrap();
        let third = service
            .enqueue_upload(
                "session-1".to_string(),
                third_path.to_string_lossy().to_string(),
                "/tmp".to_string(),
                ConflictStrategy::Reject,
                app.handle().clone(),
            )
            .expect("第三个上传应正常入队");
        assert_eq!(
            wait_for_terminal(&service, &third.task_id),
            SftpTaskStatus::Done,
            "建连失败后下一次传输应重建连接并完成"
        );
        gate.open();
        assert_eq!(
            wait_for_terminal(&service, &first.task_id),
            SftpTaskStatus::Done
        );
        assert_eq!(
            transfer_attempts.load(Ordering::SeqCst),
            3,
            "三次传输各尝试建连：成功、失败、重建成功"
        );
        let _ = std::fs::remove_file(&first_path);
        let _ = std::fs::remove_file(&second_path);
        let _ = std::fs::remove_file(&third_path);
    }

    /// Session 关闭释放全部传输连接：busy 连接在归还时释放，等待 checkout 的 worker 被唤醒终止。
    #[tokio::test(flavor = "multi_thread")]
    async fn session_close_releases_all_transfer_connections_and_waiters() {
        use crate::core::ssh_transport::test_support::{Gate, GatedCreateSftp, counted_sftp};
        use tauri::test::mock_app;

        let app = mock_app();
        let live = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let gates: Arc<std::sync::Mutex<Vec<Arc<Gate>>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));
        let live_for_connector = live.clone();
        let gates_for_connector = gates.clone();
        let service = SftpService::with_connector(move |_, role| match role {
            SftpRole::Control => Ok(memory_sftp(Vec::new())),
            SftpRole::Transfer => {
                let gate = Gate::new();
                gates_for_connector.lock().unwrap().push(gate.clone());
                Ok(counted_sftp(
                    GatedCreateSftp { released: gate },
                    live_for_connector.clone(),
                ))
            }
        });
        service.register_session("session-1".to_string(), make_host());

        let mut local_paths = Vec::new();
        for index in 0..6 {
            let path = std::env::temp_dir().join(format!(
                "titan-close-pool-{}-{}.bin",
                Uuid::new_v4(),
                index
            ));
            std::fs::write(&path, b"data").unwrap();
            local_paths.push(path);
        }
        let tasks: Vec<TransferTask> = local_paths
            .iter()
            .map(|path| {
                service
                    .enqueue_upload(
                        "session-1".to_string(),
                        path.to_string_lossy().to_string(),
                        "/tmp".to_string(),
                        ConflictStrategy::Reject,
                        app.handle().clone(),
                    )
                    .expect("上传应正常入队")
            })
            .collect();
        wait_until(
            || {
                let snapshot = service.task_snapshot("session-1");
                let running = snapshot
                    .iter()
                    .filter(|task| task.status == SftpTaskStatus::Running)
                    .count();
                let pending = snapshot
                    .iter()
                    .filter(|task| task.status == SftpTaskStatus::Pending)
                    .count();
                (running == 5 && pending == 1).then_some(())
            },
            Duration::from_secs(5),
        )
        .expect("五路 Running、一路 Pending");
        assert_eq!(live.load(Ordering::SeqCst), 5);

        // 关闭 Session：等待 checkout 的 worker 被唤醒终止；忙碌任务保留至真实终态。
        service.cleanup_session("session-1", &app.handle().clone());
        assert!(!service.has_session("session-1"));
        assert!(
            !service.tasks.lock().unwrap().is_empty(),
            "关闭后 busy worker 应保留任务直到最终结案"
        );

        // 放行 busy 传输：capability 全部释放
        let gates_snapshot = gates.lock().unwrap().clone();
        for gate in &gates_snapshot {
            gate.open();
        }
        wait_until(
            || {
                (live.load(Ordering::SeqCst) == 0 && service.task_snapshot("session-1").is_empty())
                    .then_some(())
            },
            Duration::from_secs(5),
        )
        .expect("关闭后 worker 应结案并释放全部传输连接");
        for path in &local_paths {
            let _ = std::fs::remove_file(path);
        }
        let _ = tasks;
    }

    /// 后台回收线程：额外连接空闲超时后自动释放，无需新的传输活动触发。
    #[tokio::test(flavor = "multi_thread")]
    async fn background_reaper_releases_idle_extra_without_new_activity() {
        use crate::core::ssh_transport::test_support::{Gate, counted_sftp, gated_in_memory_sftp};
        use tauri::test::mock_app;

        let app = mock_app();
        let live = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let gates: Arc<std::sync::Mutex<Vec<Arc<Gate>>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));
        let live_for_connector = live.clone();
        let gates_for_connector = gates.clone();
        let service = SftpService::with_connector_clock_timeout(
            move |_, role| match role {
                SftpRole::Control => Ok(memory_sftp(Vec::new())),
                SftpRole::Transfer => {
                    let gate = Gate::new();
                    gates_for_connector.lock().unwrap().push(gate.clone());
                    Ok(counted_sftp(
                        gated_in_memory_sftp(&[], gate, false),
                        live_for_connector.clone(),
                    ))
                }
            },
            TransferClock::system(),
            Duration::from_millis(500), // 测试用短阈值；生产阈值为 60 秒
        );
        service.register_session("session-1".to_string(), make_host());

        // 两个上传并发入队并在放行门前同时阻塞：池按需建立两条连接（基础 + 额外）
        let mut local_paths = Vec::new();
        let mut tasks = Vec::new();
        for index in 0..2 {
            let path =
                std::env::temp_dir().join(format!("titan-reaper-{}-{}.bin", Uuid::new_v4(), index));
            std::fs::write(&path, b"data").unwrap();
            let task = service
                .enqueue_upload(
                    "session-1".to_string(),
                    path.to_string_lossy().to_string(),
                    "/tmp".to_string(),
                    ConflictStrategy::Reject,
                    app.handle().clone(),
                )
                .expect("上传应正常入队");
            tasks.push(task);
            local_paths.push(path);
        }
        wait_until(
            || {
                let snapshot = service.task_snapshot("session-1");
                let running = snapshot
                    .iter()
                    .filter(|task| task.status == SftpTaskStatus::Running)
                    .count();
                (running == 2).then_some(())
            },
            Duration::from_secs(5),
        )
        .expect("两路传输应同时运行");
        assert_eq!(live.load(Ordering::SeqCst), 2, "两路运行各持一条连接");

        // 放行完成传输后两条连接空闲；无新传输活动：
        // 后台回收线程在阈值后自动释放额外连接，只保留基础一条
        let gates_snapshot = gates.lock().unwrap().clone();
        for gate in &gates_snapshot {
            gate.open();
        }
        for task in &tasks {
            assert_eq!(
                wait_for_terminal(&service, &task.task_id),
                SftpTaskStatus::Done
            );
        }
        wait_until(
            || (live.load(Ordering::SeqCst) == 1).then_some(()),
            Duration::from_secs(5),
        )
        .expect("后台线程应在空闲超时后自动回收额外连接");
        for path in &local_paths {
            let _ = std::fs::remove_file(path);
        }
    }

    // ─── 跨 Session 公平调度 contract ───────────────────────────────────────

    /// 构造 host 字段可区分的测试主机：connector 按 host 名把传输门分桶到各 Session，
    /// 供多 Session contract 测试按 Session 精确控制完成时序。
    fn make_named_host(name: &str) -> HostConfig {
        let mut host = make_host();
        host.host = name.to_string();
        host
    }

    /// 全局 20 条边界 contract：5 个 Session 各 5 路（合计 25 个任务），
    /// 跨 Session 最多 20 个 Running，其余 5 个保持 Pending 等待全局名额；
    /// 25 个任务各持一条 Session 连接（等待全局名额不提前占用 permit）。
    #[tokio::test(flavor = "multi_thread")]
    async fn global_cap_limits_twenty_running_transfers_across_sessions() {
        use crate::core::ssh_transport::test_support::{Gate, counted_sftp, gated_in_memory_sftp};
        use tauri::test::mock_app;

        let app = mock_app();
        let live = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let gates: Arc<std::sync::Mutex<Vec<Arc<Gate>>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));
        let live_for_connector = live.clone();
        let gates_for_connector = gates.clone();
        let service = SftpService::with_connector(move |_, role| match role {
            SftpRole::Control => Ok(empty_sftp()),
            SftpRole::Transfer => {
                let gate = Gate::new();
                gates_for_connector.lock().unwrap().push(gate.clone());
                Ok(counted_sftp(
                    gated_in_memory_sftp(&[], gate, false),
                    live_for_connector.clone(),
                ))
            }
        });

        let mut tasks = Vec::new();
        let mut local_paths = Vec::new();
        for session_index in 0..5 {
            let session_id = format!("session-{}", session_index);
            service.register_session(session_id.clone(), make_host());
            for file_index in 0..5 {
                let path = std::env::temp_dir().join(format!(
                    "titan-global-{}-{}-{}.bin",
                    Uuid::new_v4(),
                    session_index,
                    file_index
                ));
                std::fs::write(&path, b"data").unwrap();
                let task = service
                    .enqueue_upload(
                        session_id.clone(),
                        path.to_string_lossy().to_string(),
                        "/tmp".to_string(),
                        ConflictStrategy::Reject,
                        app.handle().clone(),
                    )
                    .expect("上传应正常入队");
                tasks.push(task);
                local_paths.push(path);
            }
        }

        // 20 个 Running + 5 个 Pending：全局边界与 Session 内五路边界同时成立
        wait_until(
            || {
                let snapshot: Vec<TransferTask> = (0..5)
                    .flat_map(|index| service.task_snapshot(&format!("session-{}", index)))
                    .collect();
                let running = snapshot
                    .iter()
                    .filter(|task| task.status == SftpTaskStatus::Running)
                    .count();
                let pending = snapshot
                    .iter()
                    .filter(|task| task.status == SftpTaskStatus::Pending)
                    .count();
                (running == 20 && pending == 5).then_some(())
            },
            Duration::from_secs(5),
        )
        .expect("全局上限下应恰好 20 个 Running 与 5 个 Pending");
        wait_until(
            || (live.load(Ordering::SeqCst) == 25).then_some(()),
            Duration::from_secs(5),
        )
        .expect("25 个任务各持一条 Session 传输连接，等待全局名额不得提前占用 permit");

        let gates_snapshot = gates.lock().unwrap().clone();
        for gate in &gates_snapshot {
            gate.open();
        }
        for task in &tasks {
            assert_eq!(
                wait_for_terminal(&service, &task.task_id),
                SftpTaskStatus::Done
            );
        }
        for path in &local_paths {
            let _ = std::fs::remove_file(path);
        }
    }

    /// 取消 Session 队列中的 Pending 任务 contract：立即移出 FIFO 队列并迁移
    /// Cancelled（不等待任何名额释放），后续等待任务保持原 FIFO 顺序启动。
    #[tokio::test(flavor = "multi_thread")]
    async fn cancelling_pending_task_removes_from_queue_and_preserves_fifo() {
        use crate::core::ssh_transport::test_support::{
            Gate, gated_in_memory_sftp, in_memory_sftp_transport,
        };
        use tauri::test::mock_app;

        let app = mock_app();
        let gates: Arc<std::sync::Mutex<Vec<Arc<Gate>>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));
        let running_events: Arc<std::sync::Mutex<Vec<String>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));
        let running_events_for_listener = running_events.clone();
        {
            use tauri::Listener;
            app.listen("sftp:task_status", move |event| {
                let payload: SftpTaskStatusEvent =
                    serde_json::from_str(event.payload()).expect("payload 应为结构化状态事件");
                if payload.status == SftpTaskStatus::Running {
                    running_events_for_listener
                        .lock()
                        .unwrap()
                        .push(payload.task_id);
                }
            });
        }
        let gates_for_connector = gates.clone();
        let service = SftpService::with_connector(move |_, role| match role {
            SftpRole::Control => Ok(memory_sftp(Vec::new())),
            SftpRole::Transfer => {
                let gate = Gate::new();
                gates_for_connector.lock().unwrap().push(gate.clone());
                Ok(in_memory_sftp_transport(&gated_in_memory_sftp(
                    &[],
                    gate,
                    false,
                )))
            }
        });
        service.register_session("session-1".to_string(), make_host());

        // 八个任务：前五个阻塞占满五路，后三个按 Session 内 FIFO 等待
        let mut local_paths = Vec::new();
        for index in 0..8 {
            let path = std::env::temp_dir().join(format!(
                "titan-cancel-queue-{}-{}.bin",
                Uuid::new_v4(),
                index
            ));
            std::fs::write(&path, b"data").unwrap();
            local_paths.push(path);
        }
        let tasks: Vec<TransferTask> = local_paths
            .iter()
            .map(|path| {
                service
                    .enqueue_upload(
                        "session-1".to_string(),
                        path.to_string_lossy().to_string(),
                        "/tmp".to_string(),
                        ConflictStrategy::Reject,
                        app.handle().clone(),
                    )
                    .expect("上传应正常入队")
            })
            .collect();
        let parked_ids = wait_until(
            || {
                let snapshot = service.task_snapshot("session-1");
                let running = snapshot
                    .iter()
                    .filter(|task| task.status == SftpTaskStatus::Running)
                    .count();
                let pending: Vec<String> = snapshot
                    .iter()
                    .filter(|task| task.status == SftpTaskStatus::Pending)
                    .map(|task| task.task_id.clone())
                    .collect();
                (running == 5 && pending.len() == 3).then_some(pending)
            },
            Duration::from_secs(5),
        )
        .expect("五路 Running、三路 Pending");
        let mut pending_in_order = parked_ids.clone();
        pending_in_order.sort_by_key(|task_id| {
            tasks
                .iter()
                .position(|task| &task.task_id == task_id)
                .expect("等待任务必须来自本测试的入队")
        });

        // 取消队首等待任务：不得释放任何运行中的名额，任务立即迁移 Cancelled
        let cancelled_id = pending_in_order[0].clone();
        service
            .cancel_task(&cancelled_id)
            .expect("取消 Pending 任务应成功");
        assert_eq!(
            wait_for_terminal(&service, &cancelled_id),
            SftpTaskStatus::Cancelled,
            "取消的 Pending 任务应立即迁移到 Cancelled，不等待名额释放"
        );
        let snapshot = service.task_snapshot("session-1");
        assert_eq!(
            snapshot
                .iter()
                .filter(|task| task.status == SftpTaskStatus::Running)
                .count(),
            5,
            "取消不得影响正在运行的五路传输"
        );
        assert_eq!(
            snapshot
                .iter()
                .filter(|task| task.status == SftpTaskStatus::Pending)
                .count(),
            2,
            "队列只剩两个等待任务"
        );

        // 释放一条连接：剩余等待任务按原 FIFO（跳过已取消任务）依次启动并级联完成
        let gates_snapshot = gates.lock().unwrap().clone();
        gates_snapshot[0].open();
        for task_id in &pending_in_order[1..] {
            assert_eq!(
                wait_for_terminal(&service, task_id),
                SftpTaskStatus::Done,
                "剩余等待任务应级联完成"
            );
        }
        let observed_order: Vec<usize> = {
            let events = running_events.lock().unwrap();
            pending_in_order[1..]
                .iter()
                .map(|task_id| {
                    events
                        .iter()
                        .position(|event_id| event_id == task_id)
                        .expect("等待任务应有 Running 事件")
                })
                .collect()
        };
        assert!(
            observed_order[0] < observed_order[1],
            "取消不影响后续 FIFO 顺序，实际 Running 顺序: {:?}",
            observed_order
        );

        for gate in gates_snapshot.iter().skip(1) {
            gate.open();
        }
        for task in &tasks {
            if task.task_id != cancelled_id {
                assert_eq!(
                    wait_for_terminal(&service, &task.task_id),
                    SftpTaskStatus::Done
                );
            }
        }
        for path in &local_paths {
            let _ = std::fs::remove_file(path);
        }
    }

    /// 取消正在等待全局 permit 的 Pending 任务 contract：立即迁移 Cancelled 并
    /// 归还 Session 传输连接，无需等待任何 permit 释放；其他 Session 的传输不受影响。
    #[tokio::test(flavor = "multi_thread")]
    async fn cancelling_task_waiting_for_global_permit_releases_session_slot() {
        use crate::core::ssh_transport::test_support::{Gate, counted_sftp, gated_in_memory_sftp};
        use tauri::test::mock_app;

        let app = mock_app();
        let live = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let connects = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let gates: Arc<std::sync::Mutex<Vec<Arc<Gate>>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));
        let live_for_connector = live.clone();
        let connects_for_connector = connects.clone();
        let gates_for_connector = gates.clone();
        let service = SftpService::with_connector(move |_, role| match role {
            SftpRole::Control => Ok(empty_sftp()),
            SftpRole::Transfer => {
                connects_for_connector.fetch_add(1, Ordering::SeqCst);
                let gate = Gate::new();
                gates_for_connector.lock().unwrap().push(gate.clone());
                Ok(counted_sftp(
                    gated_in_memory_sftp(&[], gate, false),
                    live_for_connector.clone(),
                ))
            }
        });

        // 四个 Session 各五路，占满全局 20 个 permit
        let mut tasks = Vec::new();
        let mut local_paths = Vec::new();
        for session_index in 0..4 {
            let session_id = format!("session-{}", session_index);
            service.register_session(session_id.clone(), make_host());
            for file_index in 0..5 {
                let path = std::env::temp_dir().join(format!(
                    "titan-cancel-permit-{}-{}-{}.bin",
                    Uuid::new_v4(),
                    session_index,
                    file_index
                ));
                std::fs::write(&path, b"data").unwrap();
                let task = service
                    .enqueue_upload(
                        session_id.clone(),
                        path.to_string_lossy().to_string(),
                        "/tmp".to_string(),
                        ConflictStrategy::Reject,
                        app.handle().clone(),
                    )
                    .expect("上传应正常入队");
                tasks.push(task);
                local_paths.push(path);
            }
        }
        wait_until(
            || {
                let snapshot: Vec<TransferTask> = (0..4)
                    .flat_map(|index| service.task_snapshot(&format!("session-{}", index)))
                    .collect();
                let running = snapshot
                    .iter()
                    .filter(|task| task.status == SftpTaskStatus::Running)
                    .count();
                (running == 20).then_some(())
            },
            Duration::from_secs(5),
        )
        .expect("四个 Session 五路应占满全局 20 个名额");

        // 第五个 Session 的任务取得 Session 名额后等待全局 permit，保持 Pending
        service.register_session("session-4".to_string(), make_host());
        let e_path =
            std::env::temp_dir().join(format!("titan-cancel-permit-e-{}.bin", Uuid::new_v4()));
        std::fs::write(&e_path, b"data").unwrap();
        let e_task = service
            .enqueue_upload(
                "session-4".to_string(),
                e_path.to_string_lossy().to_string(),
                "/tmp".to_string(),
                ConflictStrategy::Reject,
                app.handle().clone(),
            )
            .expect("上传应正常入队");
        wait_until(
            || {
                let snapshot = service.task_snapshot("session-4");
                (snapshot.len() == 1 && snapshot[0].status == SftpTaskStatus::Pending).then_some(())
            },
            Duration::from_secs(5),
        )
        .expect("等待全局 permit 的任务应保持 Pending");
        wait_until(
            || (live.load(Ordering::SeqCst) == 21).then_some(()),
            Duration::from_secs(5),
        )
        .expect("等待全局 permit 的任务仍应持有一条 Session 传输连接");

        // 取消：立即迁移 Cancelled 并归还连接，不得等待任何 permit 释放
        service
            .cancel_task(&e_task.task_id)
            .expect("取消等待全局 permit 的任务应成功");
        assert_eq!(
            wait_for_terminal(&service, &e_task.task_id),
            SftpTaskStatus::Cancelled
        );
        let snapshot: Vec<TransferTask> = (0..4)
            .flat_map(|index| service.task_snapshot(&format!("session-{}", index)))
            .collect();
        assert_eq!(
            snapshot
                .iter()
                .filter(|task| task.status == SftpTaskStatus::Running)
                .count(),
            20,
            "取消不得影响其他 Session 的传输"
        );
        assert_eq!(
            connects.load(Ordering::SeqCst),
            21,
            "20 个 Running 与 E 各建一条传输连接"
        );

        // 放行一路运行传输释放一个 permit；session-4 的后续任务复用取消归还的
        // 空闲连接完成，不得新建传输连接
        let gates_snapshot = gates.lock().unwrap().clone();
        gates_snapshot[0].open();
        let e2_path =
            std::env::temp_dir().join(format!("titan-cancel-permit-e2-{}.bin", Uuid::new_v4()));
        std::fs::write(&e2_path, b"data").unwrap();
        let e2_task = service
            .enqueue_upload(
                "session-4".to_string(),
                e2_path.to_string_lossy().to_string(),
                "/tmp".to_string(),
                ConflictStrategy::Reject,
                app.handle().clone(),
            )
            .expect("后续上传应正常入队");
        gates_snapshot[20].open();
        assert_eq!(
            wait_for_terminal(&service, &e2_task.task_id),
            SftpTaskStatus::Done,
            "后续任务应复用取消归还的连接完成"
        );
        assert_eq!(
            connects.load(Ordering::SeqCst),
            21,
            "后续任务不得新建传输连接"
        );

        for gate in gates_snapshot.iter().skip(1) {
            gate.open();
        }
        for task in &tasks {
            assert_eq!(
                wait_for_terminal(&service, &task.task_id),
                SftpTaskStatus::Done
            );
        }
        for path in &local_paths {
            let _ = std::fs::remove_file(path);
        }
        let _ = std::fs::remove_file(&e_path);
        let _ = std::fs::remove_file(&e2_path);
    }

    /// 无饥饿 contract：积压 Session 中等待 Session 名额的任务不得占用全局 permit；
    /// 释放出的 permit 必须交给已持有 Session 名额的任务，而不是积压任务。
    #[tokio::test(flavor = "multi_thread")]
    async fn session_backlog_does_not_grab_global_permit_ahead_of_ready_task() {
        use crate::core::ssh_transport::test_support::{Gate, counted_sftp, gated_in_memory_sftp};
        use std::collections::HashMap;
        use tauri::test::mock_app;

        let app = mock_app();
        let live = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let gates_by_session: Arc<std::sync::Mutex<HashMap<String, Vec<Arc<Gate>>>>> =
            Arc::new(std::sync::Mutex::new(HashMap::new()));
        let live_for_connector = live.clone();
        let gates_for_connector = gates_by_session.clone();
        let service = SftpService::with_connector(move |host, role| match role {
            SftpRole::Control => Ok(empty_sftp()),
            SftpRole::Transfer => {
                let gate = Gate::new();
                gates_for_connector
                    .lock()
                    .unwrap()
                    .entry(host.host.clone())
                    .or_default()
                    .push(gate.clone());
                Ok(counted_sftp(
                    gated_in_memory_sftp(&[], gate, false),
                    live_for_connector.clone(),
                ))
            }
        });

        // A 六路（含一路积压），B/C/D 各五路：合计 20 个 Running 占满全局名额
        let mut tasks = Vec::new();
        let mut local_paths = Vec::new();
        let mut enqueue = |session_id: &str, index: usize| {
            let path = std::env::temp_dir().join(format!(
                "titan-starve-{}-{}-{}.bin",
                Uuid::new_v4(),
                session_id,
                index
            ));
            std::fs::write(&path, b"data").unwrap();
            let task = service
                .enqueue_upload(
                    session_id.to_string(),
                    path.to_string_lossy().to_string(),
                    "/tmp".to_string(),
                    ConflictStrategy::Reject,
                    app.handle().clone(),
                )
                .expect("上传应正常入队");
            local_paths.push(path);
            tasks.push(task);
        };
        service.register_session("session-a".to_string(), make_named_host("session-a"));
        for index in 0..6 {
            enqueue("session-a", index);
        }
        for session_id in ["session-b", "session-c", "session-d"] {
            service.register_session(session_id.to_string(), make_named_host(session_id));
            for index in 0..5 {
                enqueue(session_id, index);
            }
        }

        // 第五个 Session 的任务持有 Session 名额，等待全局 permit
        service.register_session("session-e".to_string(), make_named_host("session-e"));
        enqueue("session-e", 0);

        // A 的积压任务（第 6 路）保持 Pending；E 的任务持名额等 permit 也保持 Pending
        let backlog_id = wait_until(
            || {
                let snapshot_a = service.task_snapshot("session-a");
                let running_a = snapshot_a
                    .iter()
                    .filter(|task| task.status == SftpTaskStatus::Running)
                    .count();
                let pending_a: Vec<String> = snapshot_a
                    .iter()
                    .filter(|task| task.status == SftpTaskStatus::Pending)
                    .map(|task| task.task_id.clone())
                    .collect();
                (running_a == 5 && pending_a.len() == 1).then_some(pending_a.into_iter().next())
            },
            Duration::from_secs(5),
        )
        .expect("A 应有五路 Running、一路积压")
        .expect("应存在积压任务");

        // 放行 B 的一路传输：释放的 permit 必须交给 E（已持有 Session 名额），
        // 而不是仍在等待 Session 名额的 A 积压任务
        {
            let gates = gates_by_session.lock().unwrap();
            gates["session-b"][0].open();
        }
        let e_task_id = service.task_snapshot("session-e")[0].task_id.clone();
        wait_until(
            || {
                let status = service
                    .tasks
                    .lock()
                    .unwrap()
                    .get(&e_task_id)
                    .map(|task| task.status.clone());
                (status == Some(SftpTaskStatus::Running)).then_some(())
            },
            Duration::from_secs(5),
        )
        .expect("释放的 permit 应交给已持有 Session 名额的任务");
        let backlog_status = service
            .tasks
            .lock()
            .unwrap()
            .get(&backlog_id)
            .map(|task| task.status.clone());
        assert_eq!(
            backlog_status,
            Some(SftpTaskStatus::Pending),
            "A 的积压任务不得抢占全局 permit"
        );

        // 放行全部：积压任务在 A 放行后依次取得名额并完成
        let gates_snapshot: Vec<Arc<Gate>> = {
            let gates = gates_by_session.lock().unwrap();
            gates.values().flatten().cloned().collect()
        };
        for gate in &gates_snapshot {
            gate.open();
        }
        for task in &tasks {
            assert_eq!(
                wait_for_terminal(&service, &task.task_id),
                SftpTaskStatus::Done
            );
        }
        for path in &local_paths {
            let _ = std::fs::remove_file(path);
        }
    }

    /// Session 关闭 contract（多 Session）：关闭释放 A 的全部 Session 与全局名额，
    /// 积压任务随 registry 整体移除且迟到 worker 不得重新启动；
    /// 释放的全局 permit 供其他 Session 已持有 Session 名额的任务继续运行。
    #[tokio::test(flavor = "multi_thread")]
    async fn session_close_releases_global_permits_and_late_tasks_do_not_restart() {
        use crate::core::ssh_transport::test_support::{Gate, counted_sftp, gated_in_memory_sftp};
        use std::collections::HashMap;
        use tauri::test::mock_app;

        let app = mock_app();
        let live = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let gates_by_session: Arc<std::sync::Mutex<HashMap<String, Vec<Arc<Gate>>>>> =
            Arc::new(std::sync::Mutex::new(HashMap::new()));
        let live_for_connector = live.clone();
        let gates_for_connector = gates_by_session.clone();
        let service = SftpService::with_connector(move |host, role| match role {
            SftpRole::Control => Ok(empty_sftp()),
            SftpRole::Transfer => {
                let gate = Gate::new();
                gates_for_connector
                    .lock()
                    .unwrap()
                    .entry(host.host.clone())
                    .or_default()
                    .push(gate.clone());
                Ok(counted_sftp(
                    gated_in_memory_sftp(&[], gate, false),
                    live_for_connector.clone(),
                ))
            }
        });

        // A 六路（含一路积压），B/C/D 各五路：20 个 Running 占满全局名额
        let mut tasks = Vec::new();
        let mut local_paths = Vec::new();
        let mut enqueue = |session_id: &str, index: usize| {
            let path = std::env::temp_dir().join(format!(
                "titan-close-global-{}-{}-{}.bin",
                Uuid::new_v4(),
                session_id,
                index
            ));
            std::fs::write(&path, b"data").unwrap();
            let task = service
                .enqueue_upload(
                    session_id.to_string(),
                    path.to_string_lossy().to_string(),
                    "/tmp".to_string(),
                    ConflictStrategy::Reject,
                    app.handle().clone(),
                )
                .expect("上传应正常入队");
            local_paths.push(path);
            tasks.push(task);
        };
        service.register_session("session-a".to_string(), make_named_host("session-a"));
        for index in 0..6 {
            enqueue("session-a", index);
        }
        for session_id in ["session-b", "session-c", "session-d"] {
            service.register_session(session_id.to_string(), make_named_host(session_id));
            for index in 0..5 {
                enqueue(session_id, index);
            }
        }
        service.register_session("session-e".to_string(), make_named_host("session-e"));
        enqueue("session-e", 0);

        // 稳定状态：A 五路 Running 一路积压，E 持 Session 名额等待全局 permit
        wait_until(
            || {
                let snapshot_a = service.task_snapshot("session-a");
                let running_a = snapshot_a
                    .iter()
                    .filter(|task| task.status == SftpTaskStatus::Running)
                    .count();
                let pending_a = snapshot_a
                    .iter()
                    .filter(|task| task.status == SftpTaskStatus::Pending)
                    .count();
                let snapshot_e = service.task_snapshot("session-e");
                let pending_e = snapshot_e
                    .iter()
                    .filter(|task| task.status == SftpTaskStatus::Pending)
                    .count();
                (running_a == 5 && pending_a == 1 && pending_e == 1).then_some(())
            },
            Duration::from_secs(5),
        )
        .expect("关闭前应稳定在 A 五路运行一路积压、E 等待全局 permit");
        wait_until(
            || (live.load(Ordering::SeqCst) == 21).then_some(()),
            Duration::from_secs(5),
        )
        .expect("20 个 Running 与持名额等待的 E 各持一条连接，A 积压任务无连接");

        // 关闭 A：禁止新 checkout，忙碌任务保留至实际终态。
        service.cleanup_session("session-a", &app.handle().clone());
        assert!(!service.has_session("session-a"));
        assert!(
            !service.task_snapshot("session-a").is_empty(),
            "关闭后 A 的忙碌任务应等待实际终态"
        );

        // 放行 A 的 busy 传输：归还连接（池已关闭直接释放）并释放全局 permit
        {
            let gates = gates_by_session.lock().unwrap();
            for gate in &gates["session-a"] {
                gate.open();
            }
        }
        wait_until(
            || (live.load(Ordering::SeqCst) == 16).then_some(()),
            Duration::from_secs(5),
        )
        .expect("关闭后 A 的传输连接应全部释放");

        // 释放出的全局 permit 交给 E；E 运行完成后其余传输继续完成
        let e_task_id = service.task_snapshot("session-e")[0].task_id.clone();
        wait_until(
            || {
                let status = service
                    .tasks
                    .lock()
                    .unwrap()
                    .get(&e_task_id)
                    .map(|task| task.status.clone());
                (status == Some(SftpTaskStatus::Running)).then_some(())
            },
            Duration::from_secs(5),
        )
        .expect("A 释放的全局 permit 应交给等待中的 E");

        let gates_snapshot: Vec<Arc<Gate>> = {
            let gates = gates_by_session.lock().unwrap();
            gates.values().flatten().cloned().collect()
        };
        for gate in &gates_snapshot {
            gate.open();
        }
        for task in &tasks {
            if task.session_id != "session-a" {
                assert_eq!(
                    wait_for_terminal(&service, &task.task_id),
                    SftpTaskStatus::Done
                );
            }
        }
        assert!(
            service.task_snapshot("session-a").is_empty(),
            "迟到 worker 不得重新启动已关闭 Session 的任务"
        );

        // 关闭其余 Session：各池释放全部空闲连接
        for session_id in ["session-b", "session-c", "session-d", "session-e"] {
            service.cleanup_session(session_id, &app.handle().clone());
        }
        wait_until(
            || (live.load(Ordering::SeqCst) == 0).then_some(()),
            Duration::from_secs(5),
        )
        .expect("全部 Session 关闭后传输连接应全部释放");
        for path in &local_paths {
            let _ = std::fs::remove_file(path);
        }
    }

    /// 传输连接池不变量：每 Session 最多 5 条，空闲回收阈值 60 秒；
    /// “基础保留一条”由回收行为 contract（回收后恰好剩一条连接）覆盖。
    #[test]
    fn transfer_pool_capacity_and_recycle_constants() {
        assert_eq!(MAX_TRANSFER_CONNECTIONS_PER_SESSION, 5);
        assert_eq!(TRANSFER_IDLE_TIMEOUT, Duration::from_secs(60));
    }

    /// 空闲回收纯策略：未满 60 秒不回收，达到 60 秒立即回收。
    #[test]
    fn idle_expiry_policy_uses_sixty_second_boundary() {
        let now = std::time::Instant::now();
        let timeout = TRANSFER_IDLE_TIMEOUT;
        assert!(!is_idle_expired(
            now - timeout + Duration::from_millis(1),
            now,
            timeout
        ));
        assert!(is_idle_expired(now - timeout, now, timeout));
        assert!(is_idle_expired(
            now - timeout - Duration::from_millis(1),
            now,
            timeout
        ));
    }

    /// 构造使用内存 SFTP adapter 的测试 module。
    fn make_service() -> SftpService {
        SftpService::with_connector(|_, _| Ok(empty_sftp()))
    }

    // ─── 基础结构测试 ───────────────────────────────────────────────────────

    /// 验证 register_session 后 handles 中存在对应条目
    #[test]
    fn register_session_stores_handle() {
        let service = make_service();
        service.register_session("session-1".to_string(), make_host());
        assert!(service.has_session("session-1"));
    }

    /// 验证 cancel_task 对不存在的 task_id 返回结构化 SftpTaskNotFound 错误。
    #[test]
    fn cancel_unknown_task_returns_structured_error() {
        let service = SftpService::new();
        let error = service.cancel_task("nonexistent-task-id").unwrap_err();
        assert!(
            matches!(&error, AppError::SftpTaskNotFound(id) if id.to_string() == "nonexistent-task-id"),
            "未知任务应返回 SftpTaskNotFound，实际: {:?}",
            error
        );
        let info = AppErrorInfo::from(error);
        assert_eq!(info.code, "SftpTaskNotFound");
        assert_eq!(info.detail.as_deref(), Some("nonexistent-task-id"));
    }

    /// 验证 cleanup_session 移除 session handle
    #[test]
    fn cleanup_session_removes_handle() {
        use tauri::test::mock_app;
        let app = mock_app();
        let service = make_service();
        service.register_session("session-1".to_string(), make_host());
        assert!(service.has_session("session-1"));
        service.cleanup_session("session-1", &app.handle().clone());
        assert!(!service.has_session("session-1"));
    }

    /// 验证 list_dir 对未注册 session 返回 SftpChannelError，且错误消息包含 session_id
    #[test]
    fn list_dir_unknown_session_returns_channel_error() {
        let service = SftpService::new();
        let result = service.list_dir("nonexistent", "/tmp");
        assert!(result.is_err());
        match result.unwrap_err() {
            AppError::SftpChannelError(msg) => assert!(msg.to_string().contains("nonexistent")),
            other => panic!("期望 SftpChannelError，实际: {:?}", other),
        }
    }

    // ─── 并发控制测试 ────────────────────────────────────────────────────────

    /// 验证全局 Semaphore 容量为 20（所有 session 合计的 Running 传输上限）
    /// 全局信号量会被并行测试抢占，故断言"可用 permits 不超过容量"这一不变量。
    #[test]
    fn semaphore_has_twenty_permits() {
        let sem = get_semaphore();
        assert_eq!(MAX_CONCURRENT_TRANSFERS, 20);
        assert!(sem.available_permits() <= MAX_CONCURRENT_TRANSFERS);
    }

    // ─── CancelToken 测试 ────────────────────────────────────────────────────

    /// 验证 CancelToken 初始未取消，cancel() 后 is_cancelled() 为 true
    #[test]
    fn cancel_token_lifecycle() {
        let token = CancelToken::new();
        assert!(!token.is_cancelled());
        token.cancel();
        assert!(token.is_cancelled());
    }

    /// 验证 CancelToken clone 共享同一原子标志（取消原始令牌，clone 也感知）
    #[test]
    fn cancel_token_clone_shares_state() {
        let token = CancelToken::new();
        let cloned = token.clone();
        token.cancel();
        assert!(cloned.is_cancelled(), "clone 应共享取消状态");
    }

    /// 安全发布已开始后，迟到的取消不得把实际已发布的传输改写为 Cancelled。
    #[test]
    fn cancel_token_commit_wins_over_late_cancellation() {
        let token = CancelToken::new();

        assert!(token.try_commit());
        token.cancel();

        assert!(!token.is_cancelled());
    }

    // ─── 权限格式化测试 ──────────────────────────────────────────────────────

    /// 验证 format_permissions 对 0o755 (rwxr-xr-x) 的转换
    #[test]
    fn format_permissions_rwxr_xr_x() {
        assert_eq!(format_permissions(0o755), "rwxr-xr-x");
    }

    /// 验证 format_permissions 对 0o644 (rw-r--r--) 的转换
    #[allow(non_snake_case)]
    #[test]
    fn format_permissions_rw_r__r__() {
        assert_eq!(format_permissions(0o644), "rw-r--r--");
    }

    /// 验证 format_permissions 对 0o700 (rwx------) 的转换
    #[test]
    fn format_permissions_rwx_only_owner() {
        assert_eq!(format_permissions(0o700), "rwx------");
    }

    // ─── upload 路径拼接测试 ─────────────────────────────────────────────────

    /// 验证 enqueue_upload 当 remote_path 为目录（不含尾部斜杠）时正确拼接文件名
    /// 本地文件 /tmp/deploy.sh 上传到 /var/log → 目标路径应为 /var/log/deploy.sh
    #[test]
    fn upload_remote_path_without_trailing_slash_appends_filename() {
        let remote_dir = "/var/log".to_string();
        let file_name = "deploy.sh";
        let full_remote = if remote_dir.ends_with('/') {
            format!("{}{}", remote_dir, file_name)
        } else {
            format!("{}/{}", remote_dir, file_name)
        };
        assert_eq!(full_remote, "/var/log/deploy.sh");
    }

    /// 验证 enqueue_upload 当 remote_path 含尾部斜杠时不重复斜杠
    #[test]
    fn upload_remote_path_with_trailing_slash_no_double_slash() {
        let remote_dir = "/var/log/".to_string();
        let file_name = "app.log";
        let full_remote = if remote_dir.ends_with('/') {
            format!("{}{}", remote_dir, file_name)
        } else {
            format!("{}/{}", remote_dir, file_name)
        };
        assert_eq!(full_remote, "/var/log/app.log");
    }

    // ─── 任务状态流转测试 ────────────────────────────────────────────────────

    /// 验证 cleanup_session 将 Pending 任务的取消令牌触发
    #[test]
    fn cleanup_session_cancels_pending_task_tokens() {
        use tauri::test::mock_app;
        let app = mock_app();
        let service = make_service();
        service.register_session("session-1".to_string(), make_host());

        let task_id = "task-pending-1".to_string();
        let cancel_token = CancelToken::new();
        let cloned_token = cancel_token.clone();

        service
            .handle("session-1")
            .unwrap()
            .cancel_tokens
            .lock()
            .unwrap()
            .insert(
                task_id.clone(),
                CancelEntry {
                    token: cancel_token,
                    queue_seq: 0,
                },
            );

        service.tasks.lock().unwrap().insert(
            task_id.clone(),
            TransferTask {
                task_id: task_id.clone(),
                session_id: "session-1".to_string(),
                transfer_type: TransferType::Download,
                remote_path: "/tmp/file".to_string(),
                local_path: "/local/file".to_string(),
                file_name: "file".to_string(),
                total_bytes: 1024,
                transferred_bytes: 0,
                speed_bps: 0,
                status: SftpTaskStatus::Pending,
                error: None,
                created_at: 0,
            },
        );

        service.cleanup_session("session-1", &app.handle().clone());

        assert!(
            cloned_token.is_cancelled(),
            "cleanup_session 应触发 Pending 任务的取消令牌"
        );
    }

    /// 验证 cleanup_session 将 Running 任务的取消令牌触发
    #[test]
    fn cleanup_session_cancels_running_task_tokens() {
        use tauri::test::mock_app;
        let app = mock_app();
        let service = make_service();
        service.register_session("session-1".to_string(), make_host());

        let task_id = "task-running-1".to_string();
        let cancel_token = CancelToken::new();
        let cloned_token = cancel_token.clone();

        service
            .handle("session-1")
            .unwrap()
            .cancel_tokens
            .lock()
            .unwrap()
            .insert(
                task_id.clone(),
                CancelEntry {
                    token: cancel_token,
                    queue_seq: 0,
                },
            );

        service.tasks.lock().unwrap().insert(
            task_id.clone(),
            TransferTask {
                task_id: task_id.clone(),
                session_id: "session-1".to_string(),
                transfer_type: TransferType::Upload,
                remote_path: "/remote/file".to_string(),
                local_path: "/local/file".to_string(),
                file_name: "file".to_string(),
                total_bytes: 2048,
                transferred_bytes: 512,
                speed_bps: 1024,
                status: SftpTaskStatus::Running,
                error: None,
                created_at: 0,
            },
        );

        service.cleanup_session("session-1", &app.handle().clone());

        assert!(
            cloned_token.is_cancelled(),
            "cleanup_session 应触发 Running 任务的取消令牌"
        );
    }

    /// 验证 cancel_task 触发对应任务的取消令牌
    #[test]
    fn cancel_task_triggers_cancel_token() {
        let service = make_service();
        service.register_session("session-1".to_string(), make_host());

        let task_id = "task-1".to_string();
        let cancel_token = CancelToken::new();
        let cloned_token = cancel_token.clone();

        service
            .handle("session-1")
            .unwrap()
            .cancel_tokens
            .lock()
            .unwrap()
            .insert(
                task_id.clone(),
                CancelEntry {
                    token: cancel_token,
                    queue_seq: 0,
                },
            );

        assert!(
            service.cancel_task(&task_id).is_ok(),
            "活跃任务的取消应成功"
        );

        assert!(
            cloned_token.is_cancelled(),
            "cancel_task 应触发对应任务的取消令牌"
        );
    }

    /// 验证终态任务调用 cancel_task 静默成功（令牌已不在 handles 中，registry 兜底）。
    #[test]
    fn cancel_task_on_completed_task_is_silent() {
        let service = make_service();
        service.register_session("session-1".to_string(), make_host());
        // 与真实终态一致：任务只存在于 registry，取消令牌已被移除
        service.tasks.lock().unwrap().insert(
            "task-already-done".to_string(),
            TransferTask {
                task_id: "task-already-done".to_string(),
                session_id: "session-1".to_string(),
                transfer_type: TransferType::Download,
                remote_path: "/tmp/file".to_string(),
                local_path: "/local/file".to_string(),
                file_name: "file".to_string(),
                total_bytes: 1024,
                transferred_bytes: 1024,
                speed_bps: 0,
                status: SftpTaskStatus::Done,
                error: None,
                created_at: 0,
            },
        );
        assert!(service.cancel_task("task-already-done").is_ok());
    }

    // ─── 任务状态迁移权威化测试 ──────────────────────────────────────────────

    /// 订阅 sftp:task_status 并收集结构化事件 payload，供断言 registry 与事件一致性。
    fn capture_task_status_events<R: Runtime>(
        app: &AppHandle<R>,
    ) -> Arc<std::sync::Mutex<Vec<SftpTaskStatusEvent>>> {
        use tauri::Listener;

        let captured: Arc<std::sync::Mutex<Vec<SftpTaskStatusEvent>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));
        let captured_ref = captured.clone();
        app.listen("sftp:task_status", move |event| {
            captured_ref.lock().unwrap().push(
                serde_json::from_str(event.payload())
                    .expect("事件 payload 应反序列化为结构化任务状态"),
            );
        });
        captured
    }

    /// 订阅 sftp:task_finished 并收集完整任务快照，供关闭后近期记录消费。
    fn capture_finished_task_events<R: Runtime>(
        app: &AppHandle<R>,
    ) -> Arc<std::sync::Mutex<Vec<TransferTask>>> {
        use tauri::Listener;

        let captured: Arc<std::sync::Mutex<Vec<TransferTask>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));
        let captured_ref = captured.clone();
        app.listen("sftp:task_finished", move |event| {
            captured_ref.lock().unwrap().push(
                serde_json::from_str(event.payload())
                    .expect("完成事件 payload 应为完整 TransferTask"),
            );
        });
        captured
    }

    /// 构造测试用 TransferTask 字面量（registry 直写的统一入口）。
    fn make_task(
        session_id: &str,
        task_id: &str,
        status: SftpTaskStatus,
        created_at: i64,
    ) -> TransferTask {
        TransferTask {
            task_id: task_id.to_string(),
            session_id: session_id.to_string(),
            transfer_type: TransferType::Download,
            remote_path: "/tmp/file".to_string(),
            local_path: "/local/file".to_string(),
            file_name: "file".to_string(),
            total_bytes: 1024,
            transferred_bytes: 0,
            speed_bps: 0,
            status,
            error: None,
            created_at,
        }
    }

    /// 构造已注册的传输任务，同时注册取消令牌，返回令牌供断言。
    fn insert_task(service: &SftpService, task_id: &str, status: SftpTaskStatus) -> CancelToken {
        let token = CancelToken::new();
        service
            .handle("session-1")
            .unwrap()
            .cancel_tokens
            .lock()
            .unwrap()
            .insert(
                task_id.to_string(),
                CancelEntry {
                    token: token.clone(),
                    queue_seq: 0,
                },
            );
        service.tasks.lock().unwrap().insert(
            task_id.to_string(),
            make_task("session-1", task_id, status, 0),
        );
        token
    }

    /// 轮询 registry 直到任务到达终态；超时 panic。
    fn wait_for_terminal(service: &SftpService, task_id: &str) -> SftpTaskStatus {
        for _ in 0..200 {
            let status = service
                .tasks
                .lock()
                .unwrap()
                .get(task_id)
                .map(|task| task.status.clone());
            if let Some(status) = status {
                if matches!(
                    status,
                    SftpTaskStatus::Done | SftpTaskStatus::Failed | SftpTaskStatus::Cancelled
                ) {
                    return status;
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("任务 {} 未在 2 秒内到达终态", task_id);
    }

    /// 轮询条件直到返回 Some(value) 或超时；供并发 contract 测试等待确定状态。
    fn wait_until<T>(mut condition: impl FnMut() -> Option<T>, timeout: Duration) -> Option<T> {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            if let Some(value) = condition() {
                return Some(value);
            }
            if std::time::Instant::now() >= deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// 迁移必须先更新 registry 再发布事件；终态后取消令牌被移除。
    #[test]
    fn transition_updates_registry_emits_and_removes_token_on_terminal() {
        use tauri::test::mock_app;

        let app = mock_app();
        let service = make_service();
        service.register_session("session-1".to_string(), make_host());
        let token = insert_task(&service, "task-1", SftpTaskStatus::Pending);

        assert!(service.transition_task(
            &app.handle(),
            "task-1",
            "session-1",
            SftpTaskStatus::Running,
            None
        ));
        assert_eq!(
            service.tasks.lock().unwrap().get("task-1").unwrap().status,
            SftpTaskStatus::Running
        );

        assert!(service.transition_task(
            &app.handle(),
            "task-1",
            "session-1",
            SftpTaskStatus::Done,
            None
        ));
        let task = service.tasks.lock().unwrap().get("task-1").unwrap().clone();
        assert_eq!(task.status, SftpTaskStatus::Done);
        assert!(task.error.is_none());
        assert!(
            service
                .handle("session-1")
                .unwrap()
                .cancel_tokens
                .lock()
                .unwrap()
                .get("task-1")
                .is_none(),
            "终态后取消令牌应从 handles 移除"
        );
        assert!(!token.is_cancelled());
    }

    /// 每个终态都发布完整任务快照，供已关闭 Session 的前端记录权威结果。
    #[test]
    fn terminal_transition_emits_complete_task_snapshot() {
        use tauri::test::mock_app;

        let app = mock_app();
        let service = make_service();
        service.register_session("session-1".to_string(), make_host());
        insert_task(&service, "task-finished", SftpTaskStatus::Pending);
        let finished = capture_finished_task_events(&app.handle());

        assert!(service.transition_task(
            &app.handle(),
            "task-finished",
            "session-1",
            SftpTaskStatus::Failed,
            Some(AppErrorInfo::from(AppError::SftpTransferError(
                "closed".to_string().into()
            ))),
        ));

        let events = finished.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].task_id, "task-finished");
        assert_eq!(events[0].status, SftpTaskStatus::Failed);
        assert_eq!(
            events[0].error.as_ref().map(|error| error.code.as_str()),
            Some("SftpTransferError")
        );
    }

    /// 终态任务拒绝继续迁移，且不得再发事件。
    #[test]
    fn terminal_task_rejects_further_transitions() {
        use std::sync::atomic::AtomicUsize;
        use tauri::Listener;
        use tauri::test::mock_app;

        let app = mock_app();
        let service = make_service();
        service.register_session("session-1".to_string(), make_host());
        insert_task(&service, "task-1", SftpTaskStatus::Done);
        let emitted = Arc::new(AtomicUsize::new(0));
        let emitted_ref = emitted.clone();
        app.listen("sftp:task_status", move |_| {
            emitted_ref.fetch_add(1, Ordering::Relaxed);
        });

        assert!(!service.transition_task(
            &app.handle(),
            "task-1",
            "session-1",
            SftpTaskStatus::Running,
            None
        ));
        assert_eq!(
            service.tasks.lock().unwrap().get("task-1").unwrap().status,
            SftpTaskStatus::Done
        );
        assert_eq!(emitted.load(Ordering::Relaxed), 0, "终态后不得再发事件");
    }

    /// registry 中不存在的任务迁移被拒绝且不发事件。
    #[test]
    fn transition_rejected_for_unknown_task_emits_nothing() {
        use std::sync::atomic::AtomicUsize;
        use tauri::Listener;
        use tauri::test::mock_app;

        let app = mock_app();
        let service = make_service();
        service.register_session("session-1".to_string(), make_host());
        let emitted = Arc::new(AtomicUsize::new(0));
        let emitted_ref = emitted.clone();
        app.listen("sftp:task_status", move |_| {
            emitted_ref.fetch_add(1, Ordering::Relaxed);
        });

        assert!(!service.transition_task(
            &app.handle(),
            "ghost-task",
            "session-1",
            SftpTaskStatus::Done,
            None
        ));
        assert_eq!(emitted.load(Ordering::Relaxed), 0);
    }

    /// Failed 迁移必须把结构化应用错误原样写入 registry，并在事件 payload 中携带一致副本。
    #[test]
    fn failed_transition_carries_structured_error_in_registry_and_event() {
        use tauri::test::mock_app;

        let app = mock_app();
        let service = make_service();
        service.register_session("session-1".to_string(), make_host());
        insert_task(&service, "task-1", SftpTaskStatus::Running);

        let captured = capture_task_status_events(app.handle());

        let error = AppErrorInfo {
            code: "SftpReadError".to_string(),
            detail: Some("read reset".to_string()),
            detail_key: None,
            detail_params: None,
        };
        assert!(service.transition_task(
            app.handle(),
            "task-1",
            "session-1",
            SftpTaskStatus::Failed,
            Some(error.clone()),
        ));

        let registry_task = service.tasks.lock().unwrap().get("task-1").unwrap().clone();
        assert_eq!(
            registry_task.error,
            Some(error.clone()),
            "registry 中的任务必须保留结构化错误"
        );
        let events = captured.lock().unwrap();
        assert_eq!(events.len(), 1, "Failed 迁移应恰好发布一次事件");
        assert_eq!(events[0].status, SftpTaskStatus::Failed);
        assert_eq!(
            events[0].error,
            Some(error),
            "事件 payload 必须携带与 registry 一致的结构化错误"
        );
    }

    /// cleanup_session 不得重复取消已终态任务，也不得发矛盾事件；任务随后从 registry 移除。
    #[test]
    fn cleanup_session_skips_terminal_tasks() {
        use std::sync::atomic::AtomicUsize;
        use tauri::Listener;
        use tauri::test::mock_app;

        let app = mock_app();
        let service = make_service();
        service.register_session("session-1".to_string(), make_host());
        let token = insert_task(&service, "task-1", SftpTaskStatus::Done);
        let emitted = Arc::new(AtomicUsize::new(0));
        let emitted_ref = emitted.clone();
        app.listen("sftp:task_status", move |_| {
            emitted_ref.fetch_add(1, Ordering::Relaxed);
        });

        service.cleanup_session("session-1", &app.handle().clone());

        assert!(
            !token.is_cancelled(),
            "终态任务的取消令牌不应被 cleanup 触发"
        );
        assert_eq!(
            emitted.load(Ordering::Relaxed),
            0,
            "cleanup 不应为终态任务发 Cancelled 事件"
        );
        assert!(
            !service.tasks.lock().unwrap().contains_key("task-1"),
            "cleanup 应从 registry 移除该 session 的任务"
        );
    }

    /// cleanup_session 对非终态任务：只触发令牌，worker 结案前不伪造终态事件。
    #[test]
    fn cleanup_session_cancels_active_task_with_event() {
        use std::sync::atomic::AtomicUsize;
        use tauri::Listener;
        use tauri::test::mock_app;

        let app = mock_app();
        let service = make_service();
        service.register_session("session-1".to_string(), make_host());
        let token = insert_task(&service, "task-1", SftpTaskStatus::Running);
        let emitted = Arc::new(AtomicUsize::new(0));
        let emitted_ref = emitted.clone();
        app.listen("sftp:task_status", move |_| {
            emitted_ref.fetch_add(1, Ordering::Relaxed);
        });

        service.cleanup_session("session-1", &app.handle().clone());

        assert!(token.is_cancelled(), "非终态任务的令牌应被 cleanup 触发");
        assert_eq!(emitted.load(Ordering::Relaxed), 0);
        assert!(
            service.tasks.lock().unwrap().contains_key("task-1"),
            "cleanup 后任务应保留至 worker 结案"
        );
        assert!(service.transition_task(
            &app.handle(),
            "task-1",
            "session-1",
            SftpTaskStatus::Cancelled,
            None,
        ));
        assert_eq!(emitted.load(Ordering::Relaxed), 1);
        assert!(service.tasks.lock().unwrap().is_empty());
    }

    // ─── worker 更新 registry 的全链路测试 ─────────────────────────────────

    /// 上传 worker 完成真实传输后，registry 由 worker 更新为 Done 并移除令牌。
    #[tokio::test(flavor = "multi_thread")]
    async fn upload_worker_updates_registry_to_done() {
        use crate::core::ssh_transport::test_support::{in_memory_sftp, in_memory_sftp_transport};
        use tauri::test::mock_app;

        let app = mock_app();
        let fs = in_memory_sftp(&[]);
        let fs_for_connector = fs.clone();
        let service = SftpService::with_connector(move |_, _| {
            Ok(in_memory_sftp_transport(&fs_for_connector))
        });
        service.register_session("session-1".to_string(), make_host());

        let local_path = std::env::temp_dir().join(format!("titan-upload-{}.bin", Uuid::new_v4()));
        std::fs::write(&local_path, b"hello").unwrap();
        let task = service
            .enqueue_upload(
                "session-1".to_string(),
                local_path.to_string_lossy().to_string(),
                "/tmp".to_string(),
                ConflictStrategy::Reject,
                app.handle().clone(),
            )
            .unwrap();

        assert_eq!(
            wait_for_terminal(&service, &task.task_id),
            SftpTaskStatus::Done
        );
        let registry_task = service
            .tasks
            .lock()
            .unwrap()
            .get(&task.task_id)
            .unwrap()
            .clone();
        assert_eq!(registry_task.status, SftpTaskStatus::Done);
        assert!(registry_task.error.is_none());
        assert!(
            service
                .handle("session-1")
                .unwrap()
                .cancel_tokens
                .lock()
                .unwrap()
                .get(&task.task_id)
                .is_none(),
            "worker 迁移到终态后应移除取消令牌"
        );
        let _ = std::fs::remove_file(&local_path);
    }

    /// 下载 worker 完成真实传输后，registry 由 worker 更新为 Done，内容写入本地。
    #[tokio::test(flavor = "multi_thread")]
    async fn download_worker_updates_registry_to_done() {
        use tauri::test::mock_app;

        let app = mock_app();
        let service = SftpService::with_connector(|_, _| Ok(memory_sftp(vec![7u8; 4096])));
        service.register_session("session-1".to_string(), make_host());

        let local_path =
            std::env::temp_dir().join(format!("titan-download-{}.bin", Uuid::new_v4()));
        let task = service
            .enqueue_download(
                "session-1".to_string(),
                "/remote/file.bin".to_string(),
                local_path.to_string_lossy().to_string(),
                ConflictStrategy::Reject,
                app.handle().clone(),
            )
            .unwrap();

        assert_eq!(
            wait_for_terminal(&service, &task.task_id),
            SftpTaskStatus::Done
        );
        assert_eq!(
            std::fs::metadata(&local_path).unwrap().len(),
            4096,
            "下载内容应写入本地文件"
        );
        let _ = std::fs::remove_file(&local_path);
    }

    /// 传输启动失败（远端 create 拒绝）时 worker 把 registry 更新为 Failed，
    /// 且 registry 与事件 payload 携带同一份结构化创建错误。
    #[tokio::test(flavor = "multi_thread")]
    async fn failing_worker_updates_registry_to_failed() {
        use tauri::test::mock_app;

        let app = mock_app();
        let service = make_service(); // empty_sftp：open_read/create 失败
        service.register_session("session-1".to_string(), make_host());

        let captured = capture_task_status_events(app.handle());

        let local_path = std::env::temp_dir().join(format!("titan-fail-{}.bin", Uuid::new_v4()));
        std::fs::write(&local_path, b"x").unwrap();
        let task = service
            .enqueue_upload(
                "session-1".to_string(),
                local_path.to_string_lossy().to_string(),
                "/tmp".to_string(),
                ConflictStrategy::Reject,
                app.handle().clone(),
            )
            .unwrap();

        assert_eq!(
            wait_for_terminal(&service, &task.task_id),
            SftpTaskStatus::Failed
        );
        let expected = Some(AppErrorInfo {
            code: "SftpTransferError".to_string(),
            detail: Some("unused".to_string()),
            detail_key: None,
            detail_params: None,
        });
        let registry_task = service
            .tasks
            .lock()
            .unwrap()
            .get(&task.task_id)
            .unwrap()
            .clone();
        assert_eq!(
            registry_task.error, expected,
            "registry 必须保留启动失败的具体错误"
        );
        let events = captured.lock().unwrap();
        let failed_events: Vec<&SftpTaskStatusEvent> = events
            .iter()
            .filter(|event| event.status == SftpTaskStatus::Failed)
            .collect();
        assert_eq!(failed_events.len(), 1, "启动失败应发布一次 Failed 事件");
        assert_eq!(failed_events[0].error, expected);
        let _ = std::fs::remove_file(&local_path);
    }

    /// 下载运行时读取失败：worker 保留 SftpReadError 结构化错误，且事件与 registry 一致。
    #[tokio::test(flavor = "multi_thread")]
    async fn download_read_failure_keeps_structured_read_error() {
        use tauri::test::mock_app;

        let app = mock_app();
        let service = SftpService::with_connector(|_, _| Ok(failing_read_sftp()));
        service.register_session("session-1".to_string(), make_host());

        let captured = capture_task_status_events(app.handle());

        let local_path =
            std::env::temp_dir().join(format!("titan-readfail-{}.bin", Uuid::new_v4()));
        let task = service
            .enqueue_download(
                "session-1".to_string(),
                "/remote/file.bin".to_string(),
                local_path.to_string_lossy().to_string(),
                ConflictStrategy::Reject,
                app.handle().clone(),
            )
            .unwrap();

        assert_eq!(
            wait_for_terminal(&service, &task.task_id),
            SftpTaskStatus::Failed
        );
        let registry_task = service
            .tasks
            .lock()
            .unwrap()
            .get(&task.task_id)
            .unwrap()
            .clone();
        assert_eq!(
            registry_task.error,
            Some(AppErrorInfo {
                code: "SftpReadError".to_string(),
                detail: Some("remote read reset".to_string()),
                detail_key: None,
                detail_params: None,
            }),
            "运行时读取失败必须保留 SftpReadError 与底层诊断"
        );
        assert!(
            !std::path::Path::new(&local_path).exists(),
            "读取失败后本地残留文件应被清理"
        );
        let events = captured.lock().unwrap();
        let failed_events: Vec<&SftpTaskStatusEvent> = events
            .iter()
            .filter(|event| event.status == SftpTaskStatus::Failed)
            .collect();
        assert_eq!(failed_events.len(), 1);
        assert_eq!(failed_events[0].error, registry_task.error);
    }

    /// 上传运行时写入失败：worker 保留 SftpWriteError 结构化错误，且事件与 registry 一致。
    #[tokio::test(flavor = "multi_thread")]
    async fn upload_write_failure_keeps_structured_write_error() {
        use tauri::test::mock_app;

        let app = mock_app();
        let service = SftpService::with_connector(|_, _| Ok(failing_write_sftp()));
        service.register_session("session-1".to_string(), make_host());

        let captured = capture_task_status_events(app.handle());

        let local_path =
            std::env::temp_dir().join(format!("titan-writefail-{}.bin", Uuid::new_v4()));
        std::fs::write(&local_path, b"hello").unwrap();
        let task = service
            .enqueue_upload(
                "session-1".to_string(),
                local_path.to_string_lossy().to_string(),
                "/tmp".to_string(),
                ConflictStrategy::Reject,
                app.handle().clone(),
            )
            .unwrap();

        assert_eq!(
            wait_for_terminal(&service, &task.task_id),
            SftpTaskStatus::Failed
        );
        let registry_task = service
            .tasks
            .lock()
            .unwrap()
            .get(&task.task_id)
            .unwrap()
            .clone();
        assert_eq!(
            registry_task.error,
            Some(AppErrorInfo {
                code: "SftpWriteError".to_string(),
                detail: Some("remote write reset".to_string()),
                detail_key: None,
                detail_params: None,
            }),
            "运行时写入失败必须保留 SftpWriteError 与底层诊断"
        );
        let events = captured.lock().unwrap();
        let failed_events: Vec<&SftpTaskStatusEvent> = events
            .iter()
            .filter(|event| event.status == SftpTaskStatus::Failed)
            .collect();
        assert_eq!(failed_events.len(), 1);
        assert_eq!(failed_events[0].error, registry_task.error);
        let _ = std::fs::remove_file(&local_path);
    }

    /// 下载临时文件创建失败（目标父路径是文件而非目录）：任务 Failed 且保留
    /// SftpCreateError，错误 detail 包含临时文件路径。
    #[tokio::test(flavor = "multi_thread")]
    async fn download_local_create_failure_keeps_structured_create_error() {
        use tauri::test::mock_app;

        let app = mock_app();
        let service = SftpService::with_connector(|_, _| Ok(memory_sftp(vec![7u8; 4096])));
        service.register_session("session-1".to_string(), make_host());

        // 本地目标的父路径是一个普通文件：临时文件 File::create 必然失败（ENOTDIR）
        let parent_file = std::env::temp_dir().join(format!("titan-createfail-{}", Uuid::new_v4()));
        std::fs::write(&parent_file, b"not a dir").unwrap();
        let local_path = parent_file.join("file.bin");
        let task = service
            .enqueue_download(
                "session-1".to_string(),
                "/remote/file.bin".to_string(),
                local_path.to_string_lossy().to_string(),
                ConflictStrategy::Reject,
                app.handle().clone(),
            )
            .unwrap();

        assert_eq!(
            wait_for_terminal(&service, &task.task_id),
            SftpTaskStatus::Failed
        );
        let registry_task = service
            .tasks
            .lock()
            .unwrap()
            .get(&task.task_id)
            .unwrap()
            .clone();
        let error = registry_task
            .error
            .as_ref()
            .expect("本地创建失败必须携带结构化错误");
        assert_eq!(error.code, "SftpCreateError");
        assert!(
            error
                .detail
                .as_deref()
                .is_some_and(|detail| detail.contains(".part")),
            "创建失败必须保留临时文件路径诊断"
        );
        let _ = std::fs::remove_file(&parent_file);
    }

    /// 上传本地文件打开失败（权限拒绝）：任务 Failed 且保留 SftpOpenError。
    /// root 环境不受权限位约束，无法模拟时跳过断言。
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread")]
    async fn upload_local_open_failure_keeps_structured_open_error() {
        use std::os::unix::fs::PermissionsExt;
        use tauri::test::mock_app;

        let app = mock_app();
        let service = SftpService::with_connector(|_, _| Ok(memory_sftp(Vec::new())));
        service.register_session("session-1".to_string(), make_host());

        let local_path =
            std::env::temp_dir().join(format!("titan-openfail-{}.bin", Uuid::new_v4()));
        std::fs::write(&local_path, b"hello").unwrap();
        let mut permissions = std::fs::metadata(&local_path).unwrap().permissions();
        permissions.set_mode(0o000);
        std::fs::set_permissions(&local_path, permissions).unwrap();

        // root 用户不受权限位限制，该环境无法模拟本地打开失败：清理后跳过断言
        if std::fs::File::open(&local_path).is_ok() {
            let _ = std::fs::remove_file(&local_path);
            return;
        }

        let task = service
            .enqueue_upload(
                "session-1".to_string(),
                local_path.to_string_lossy().to_string(),
                "/tmp".to_string(),
                ConflictStrategy::Reject,
                app.handle().clone(),
            )
            .unwrap();

        assert_eq!(
            wait_for_terminal(&service, &task.task_id),
            SftpTaskStatus::Failed
        );
        let registry_task = service
            .tasks
            .lock()
            .unwrap()
            .get(&task.task_id)
            .unwrap()
            .clone();
        let error = registry_task
            .error
            .as_ref()
            .expect("本地打开失败必须携带结构化错误");
        assert_eq!(error.code, "SftpOpenError");
        assert!(
            error
                .detail
                .as_deref()
                .is_some_and(|detail| !detail.is_empty()),
            "打开失败必须保留底层诊断"
        );

        // 恢复权限后清理临时文件
        let mut permissions = std::fs::metadata(&local_path).unwrap().permissions();
        permissions.set_mode(0o644);
        let _ = std::fs::set_permissions(&local_path, permissions);
        let _ = std::fs::remove_file(&local_path);
    }

    /// Reject 策略下目标已存在：发布前检查发现冲突，任务 Failed 且保留
    /// SftpTargetExists 结构化错误；原有本地文件内容与临时文件均不受破坏。
    #[tokio::test(flavor = "multi_thread")]
    async fn download_reject_conflict_fails_and_keeps_existing_file() {
        use tauri::test::mock_app;

        let app = mock_app();
        let service = SftpService::with_connector(|_, _| Ok(memory_sftp(vec![9u8; 2048])));
        service.register_session("session-1".to_string(), make_host());

        let local_path = std::env::temp_dir().join(format!("titan-reject-{}.bin", Uuid::new_v4()));
        std::fs::write(&local_path, b"original").unwrap();
        let task = service
            .enqueue_download(
                "session-1".to_string(),
                "/remote/file.bin".to_string(),
                local_path.to_string_lossy().to_string(),
                ConflictStrategy::Reject,
                app.handle().clone(),
            )
            .unwrap();

        assert_eq!(
            wait_for_terminal(&service, &task.task_id),
            SftpTaskStatus::Failed
        );
        let registry_task = service
            .tasks
            .lock()
            .unwrap()
            .get(&task.task_id)
            .unwrap()
            .clone();
        let error = registry_task
            .error
            .as_ref()
            .expect("冲突失败必须携带结构化错误");
        assert_eq!(error.code, "SftpTargetExists");
        assert_eq!(
            std::fs::read(&local_path).unwrap(),
            b"original",
            "Reject 冲突不得破坏原有本地文件"
        );
        let temp_path = download_temp_path(&local_path.to_string_lossy(), &task.task_id);
        assert!(
            !temp_path.exists(),
            "冲突失败后临时文件应被清理: {}",
            temp_path.display()
        );
        let _ = std::fs::remove_file(&local_path);
    }

    /// Overwrite 策略（用户已逐文件确认）：临时文件原子替换最终目标，
    /// 任务 Done，目标内容为远程内容，临时文件不残留。
    #[tokio::test(flavor = "multi_thread")]
    async fn download_overwrite_replaces_existing_file() {
        use tauri::test::mock_app;

        let app = mock_app();
        let service = SftpService::with_connector(|_, _| Ok(memory_sftp(vec![3u8; 4096])));
        service.register_session("session-1".to_string(), make_host());

        let local_path =
            std::env::temp_dir().join(format!("titan-overwrite-{}.bin", Uuid::new_v4()));
        std::fs::write(&local_path, b"original").unwrap();
        let task = service
            .enqueue_download(
                "session-1".to_string(),
                "/remote/file.bin".to_string(),
                local_path.to_string_lossy().to_string(),
                ConflictStrategy::Overwrite,
                app.handle().clone(),
            )
            .unwrap();

        assert_eq!(
            wait_for_terminal(&service, &task.task_id),
            SftpTaskStatus::Done
        );
        assert_eq!(
            std::fs::read(&local_path).unwrap(),
            vec![3u8; 4096],
            "确认覆盖后目标内容应为远程内容"
        );
        let temp_path = download_temp_path(&local_path.to_string_lossy(), &task.task_id);
        assert!(!temp_path.exists(), "发布成功后临时文件不应残留");
        let _ = std::fs::remove_file(&local_path);
    }

    /// 新文件（目标不存在）：Reject 策略直接发布成功，临时文件不残留。
    #[tokio::test(flavor = "multi_thread")]
    async fn download_new_file_publishes_without_temp_leftover() {
        use tauri::test::mock_app;

        let app = mock_app();
        let service = SftpService::with_connector(|_, _| Ok(memory_sftp(vec![5u8; 512])));
        service.register_session("session-1".to_string(), make_host());

        let local_path = std::env::temp_dir().join(format!("titan-new-{}.bin", Uuid::new_v4()));
        let task = service
            .enqueue_download(
                "session-1".to_string(),
                "/remote/file.bin".to_string(),
                local_path.to_string_lossy().to_string(),
                ConflictStrategy::Reject,
                app.handle().clone(),
            )
            .unwrap();

        assert_eq!(
            wait_for_terminal(&service, &task.task_id),
            SftpTaskStatus::Done
        );
        assert_eq!(std::fs::read(&local_path).unwrap(), vec![5u8; 512]);
        let temp_path = download_temp_path(&local_path.to_string_lossy(), &task.task_id);
        assert!(!temp_path.exists(), "发布成功后临时文件不应残留");
        let _ = std::fs::remove_file(&local_path);
    }

    /// 零字节远程文件：临时文件创建、刷新与发布流程仍完整执行，
    /// 目标以空文件落地且任务 Done。
    #[tokio::test(flavor = "multi_thread")]
    async fn download_zero_byte_file_publishes_empty_target() {
        use tauri::test::mock_app;

        let app = mock_app();
        let service = SftpService::with_connector(|_, _| Ok(memory_sftp(Vec::new())));
        service.register_session("session-1".to_string(), make_host());

        let local_path = std::env::temp_dir().join(format!("titan-zero-{}.bin", Uuid::new_v4()));
        let task = service
            .enqueue_download(
                "session-1".to_string(),
                "/remote/file.bin".to_string(),
                local_path.to_string_lossy().to_string(),
                ConflictStrategy::Reject,
                app.handle().clone(),
            )
            .unwrap();

        assert_eq!(
            wait_for_terminal(&service, &task.task_id),
            SftpTaskStatus::Done
        );
        assert_eq!(
            std::fs::metadata(&local_path).unwrap().len(),
            0,
            "零字节文件应发布为空的最终目标"
        );
        let temp_path = download_temp_path(&local_path.to_string_lossy(), &task.task_id);
        assert!(!temp_path.exists());
        let _ = std::fs::remove_file(&local_path);
    }

    /// 传输中取消：目标原文件保持不动，临时文件被清理，任务终态 Cancelled。
    #[tokio::test(flavor = "multi_thread")]
    async fn download_cancel_keeps_existing_file_and_cleans_temp() {
        use tauri::test::mock_app;

        let app = mock_app();
        let transfer_started = Arc::new(Barrier::new(2));
        let transfer_release = Arc::new(Barrier::new(2));
        let started_for_connector = transfer_started.clone();
        let release_for_connector = transfer_release.clone();
        let service = SftpService::with_connector(move |_, role| match role {
            SftpRole::Control => Ok(memory_sftp(vec![7u8; 4096])),
            SftpRole::Transfer => Ok(blocking_read_sftp(
                started_for_connector.clone(),
                release_for_connector.clone(),
            )),
        });
        service.register_session("session-1".to_string(), make_host());

        let local_path = std::env::temp_dir().join(format!("titan-cancel-{}.bin", Uuid::new_v4()));
        std::fs::write(&local_path, b"original").unwrap();
        let task = service
            .enqueue_download(
                "session-1".to_string(),
                "/remote/file.bin".to_string(),
                local_path.to_string_lossy().to_string(),
                ConflictStrategy::Reject,
                app.handle().clone(),
            )
            .unwrap();
        transfer_started.wait(); // 传输进入阻塞读取，临时文件已创建

        service.cancel_task(&task.task_id).unwrap();
        transfer_release.wait();

        assert_eq!(
            wait_for_terminal(&service, &task.task_id),
            SftpTaskStatus::Cancelled
        );
        assert_eq!(
            std::fs::read(&local_path).unwrap(),
            b"original",
            "取消不得破坏原有本地文件"
        );
        let temp_path = download_temp_path(&local_path.to_string_lossy(), &task.task_id);
        assert!(!temp_path.exists(), "取消后临时文件应被清理");
        let _ = std::fs::remove_file(&local_path);
    }

    /// 取消发生在创建临时文件之前：本地不产生任何写入。
    #[test]
    fn download_cancelled_before_temp_create_skips_local_write() {
        use tauri::test::mock_app;

        let app = mock_app();
        let service = SftpService::with_connector(|_, _| Ok(memory_sftp(vec![1u8; 16])));
        service.register_session("session-1".to_string(), make_host());

        let handle = service.handle("session-1").unwrap();
        let checkout = handle.transfer_pool.checkout(0).unwrap();
        let local_path =
            std::env::temp_dir().join(format!("titan-precancel-{}.bin", Uuid::new_v4()));
        let temp_path = download_temp_path(&local_path.to_string_lossy(), "task-before-temp");

        let cancel_token = CancelToken::new();
        cancel_token.cancel();
        let outcome = run_transfer_blocking(
            "task-before-temp",
            "session-1",
            "/remote/file.bin",
            &local_path.to_string_lossy(),
            16,
            &TransferType::Download,
            Some(ConflictStrategy::Reject),
            &checkout.transport,
            &cancel_token,
            app.handle(),
        );

        assert!(
            matches!(outcome, TransferOutcome::Cancelled(None)),
            "预取消应直接返回 Cancelled，实际: {:?}",
            outcome
        );
        assert!(!temp_path.exists(), "取消检查先于临时文件创建");
        assert!(!local_path.exists(), "预取消不得写入最终目标");
    }

    /// 安全发布已抢占后，即使关闭 Session 请求取消，实际发布仍必须报告 Done。
    #[test]
    fn download_committed_before_close_publishes_target() {
        use tauri::test::mock_app;

        let app = mock_app();
        let service = SftpService::with_connector(|_, _| Ok(memory_sftp(vec![9u8; 16])));
        service.register_session("session-1".to_string(), make_host());
        let checkout = service
            .handle("session-1")
            .unwrap()
            .transfer_pool
            .checkout(0)
            .unwrap();
        let local_path = std::env::temp_dir().join(format!("titan-commit-{}.bin", Uuid::new_v4()));
        let token = CancelToken::new();

        assert!(token.try_commit());
        assert!(!token.cancel(), "提交权抢占后关闭取消不得覆盖结果");
        let outcome = run_transfer_blocking(
            "task-committed",
            "session-1",
            "/remote/file.bin",
            &local_path.to_string_lossy(),
            16,
            &TransferType::Download,
            Some(ConflictStrategy::Reject),
            &checkout.transport,
            &token,
            app.handle(),
        );

        assert!(matches!(outcome, TransferOutcome::Done));
        assert_eq!(std::fs::read(&local_path).unwrap(), vec![9u8; 16]);
        let _ = std::fs::remove_file(&local_path);
    }

    /// 清理临时文件失败：错误 detail 必须包含临时文件路径。
    #[test]
    fn cleanup_download_temp_failure_includes_temp_path() {
        let temp_path = std::env::temp_dir().join(format!("titan-cleanup-fail-{}", Uuid::new_v4()));
        // 目录不能被 remove_file 删除（所有平台），强制清理失败
        std::fs::create_dir(&temp_path).unwrap();

        let error = cleanup_download_temp(&temp_path).expect_err("目录路径清理应失败");
        let detail = error.to_string();
        assert!(
            detail.contains(&temp_path.to_string_lossy().to_string()),
            "清理失败错误必须包含临时路径，实际: {}",
            detail
        );
        let _ = std::fs::remove_dir(&temp_path);
    }

    /// Overwrite 发布失败（目标为目录，平台无法安全替换）：任务 Failed 且保留
    /// SftpPublishError，原目标目录不受影响，临时文件被清理。
    #[tokio::test(flavor = "multi_thread")]
    async fn download_publish_failure_preserves_existing_target() {
        use tauri::test::mock_app;

        let app = mock_app();
        let service = SftpService::with_connector(|_, _| Ok(memory_sftp(vec![8u8; 64])));
        service.register_session("session-1".to_string(), make_host());

        let local_dir = std::env::temp_dir().join(format!("titan-publishfail-{}", Uuid::new_v4()));
        std::fs::create_dir(&local_dir).unwrap();
        let task = service
            .enqueue_download(
                "session-1".to_string(),
                "/remote/file.bin".to_string(),
                local_dir.to_string_lossy().to_string(),
                ConflictStrategy::Overwrite,
                app.handle().clone(),
            )
            .unwrap();

        assert_eq!(
            wait_for_terminal(&service, &task.task_id),
            SftpTaskStatus::Failed
        );
        let registry_task = service
            .tasks
            .lock()
            .unwrap()
            .get(&task.task_id)
            .unwrap()
            .clone();
        let error = registry_task
            .error
            .as_ref()
            .expect("发布失败必须携带结构化错误");
        assert_eq!(error.code, "SftpPublishError");
        assert!(local_dir.is_dir(), "发布失败不得破坏原目标（目录仍存在）");
        let temp_path = download_temp_path(&local_dir.to_string_lossy(), &task.task_id);
        assert!(!temp_path.exists(), "发布失败后临时文件应被清理");
        let _ = std::fs::remove_dir(&local_dir);
    }

    /// 取消后清理失败（临时文件被替换为目录）：任务仍为 Cancelled 且错误
    /// detail 包含临时路径；仅 unix（可删除打开中的文件完成替换）。
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread")]
    async fn download_cancel_cleanup_failure_reports_temp_path() {
        use tauri::test::mock_app;

        let app = mock_app();
        let transfer_started = Arc::new(Barrier::new(2));
        let transfer_release = Arc::new(Barrier::new(2));
        let started_for_connector = transfer_started.clone();
        let release_for_connector = transfer_release.clone();
        let service = SftpService::with_connector(move |_, role| match role {
            SftpRole::Control => Ok(memory_sftp(vec![7u8; 4096])),
            SftpRole::Transfer => Ok(blocking_read_sftp(
                started_for_connector.clone(),
                release_for_connector.clone(),
            )),
        });
        service.register_session("session-1".to_string(), make_host());

        let local_path =
            std::env::temp_dir().join(format!("titan-cancelfail-{}.bin", Uuid::new_v4()));
        let task = service
            .enqueue_download(
                "session-1".to_string(),
                "/remote/file.bin".to_string(),
                local_path.to_string_lossy().to_string(),
                ConflictStrategy::Reject,
                app.handle().clone(),
            )
            .unwrap();
        transfer_started.wait();

        // 传输阻塞期间把临时文件替换为目录：worker 的清理 remove_file 必然失败
        let temp_path = download_temp_path(&local_path.to_string_lossy(), &task.task_id);
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !temp_path.exists() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(temp_path.exists(), "worker 应先创建临时文件");
        std::fs::remove_file(&temp_path).unwrap();
        std::fs::create_dir(&temp_path).unwrap();

        service.cancel_task(&task.task_id).unwrap();
        transfer_release.wait();

        assert_eq!(
            wait_for_terminal(&service, &task.task_id),
            SftpTaskStatus::Cancelled
        );
        let registry_task = service
            .tasks
            .lock()
            .unwrap()
            .get(&task.task_id)
            .unwrap()
            .clone();
        let error = registry_task
            .error
            .as_ref()
            .expect("清理失败时 Cancelled 必须携带错误");
        assert_eq!(error.code, "SftpTransferError");
        let detail_key = error.detail_key.as_deref().unwrap_or_default();
        let params = error.detail_params.clone().unwrap_or_default();
        assert!(
            detail_key.contains("清理临时文件失败")
                && params
                    .iter()
                    .any(|param| param.contains(&temp_path.to_string_lossy().to_string())),
            "清理失败错误必须包含临时路径，实际: {:?}",
            error
        );
        let _ = std::fs::remove_dir(&temp_path);
    }

    /// 同步上下文（无 tokio runtime，模拟同步 Tauri command 线程）发起上传：
    /// 不得 panic，且任务最终由 worker 迁移到 Done。
    #[test]
    fn sync_context_enqueue_upload_completes_without_runtime() {
        use crate::core::ssh_transport::test_support::{in_memory_sftp, in_memory_sftp_transport};
        use tauri::test::mock_app;

        let app = mock_app();
        let fs = in_memory_sftp(&[]);
        let fs_for_connector = fs.clone();
        let service = SftpService::with_connector(move |_, _| {
            Ok(in_memory_sftp_transport(&fs_for_connector))
        });
        service.register_session("session-1".to_string(), make_host());

        let local_path = std::env::temp_dir().join(format!("titan-sync-{}.bin", Uuid::new_v4()));
        std::fs::write(&local_path, b"hello").unwrap();
        let task = service
            .enqueue_upload(
                "session-1".to_string(),
                local_path.to_string_lossy().to_string(),
                "/tmp".to_string(),
                ConflictStrategy::Reject,
                app.handle().clone(),
            )
            .unwrap();

        assert_eq!(
            wait_for_terminal(&service, &task.task_id),
            SftpTaskStatus::Done
        );
        let _ = std::fs::remove_file(&local_path);
    }

    // ─── 任务快照与队列上限测试 ──────────────────────────────────────────────

    /// 直接写入 registry 的测试任务（不经过传输 worker），created_at 可控。
    fn insert_registry_task(
        service: &SftpService,
        session_id: &str,
        task_id: &str,
        status: SftpTaskStatus,
        created_at: i64,
    ) {
        service.tasks.lock().unwrap().insert(
            task_id.to_string(),
            make_task(session_id, task_id, status, created_at),
        );
    }

    /// 快照只返回指定 Session 的任务，且按 createdAt 最新优先排序。
    #[test]
    fn task_snapshot_returns_only_requested_session_tasks_newest_first() {
        let service = make_service();
        service.register_session("session-1".to_string(), make_host());
        service.register_session("session-2".to_string(), make_host());
        insert_registry_task(&service, "session-1", "t1-old", SftpTaskStatus::Done, 1);
        insert_registry_task(&service, "session-1", "t1-new", SftpTaskStatus::Running, 2);
        insert_registry_task(&service, "session-2", "t2", SftpTaskStatus::Done, 3);

        let snapshot = service.task_snapshot("session-1");
        let ids: Vec<&str> = snapshot.iter().map(|task| task.task_id.as_str()).collect();
        assert_eq!(ids, vec!["t1-new", "t1-old"]);
    }

    /// Session 关闭后 registry 清空，快照返回空列表（前端据此清空投影）。
    #[test]
    fn task_snapshot_after_session_close_is_empty() {
        use tauri::test::mock_app;
        let app = mock_app();
        let service = make_service();
        service.register_session("session-1".to_string(), make_host());
        insert_registry_task(&service, "session-1", "t1", SftpTaskStatus::Done, 1);

        service.cleanup_session("session-1", &app.handle().clone());

        assert!(service.task_snapshot("session-1").is_empty());
    }

    /// 终态任务超过 100 条时淘汰最旧记录；Pending/Running 与其他 Session 不受影响。
    #[test]
    fn terminal_eviction_keeps_latest_100_and_protects_active_tasks() {
        use tauri::test::mock_app;
        let app = mock_app();
        let service = make_service();
        service.register_session("session-1".to_string(), make_host());
        for i in 0..100 {
            insert_registry_task(
                &service,
                "session-1",
                &format!("terminal-{}", i),
                SftpTaskStatus::Done,
                i,
            );
        }
        // 活动任务 created_at 与最旧终态相同：淘汰不得触碰
        insert_registry_task(
            &service,
            "session-1",
            "running-old",
            SftpTaskStatus::Running,
            0,
        );
        insert_registry_task(
            &service,
            "session-1",
            "pending-new",
            SftpTaskStatus::Pending,
            1000,
        );
        insert_registry_task(
            &service,
            "session-2",
            "other-terminal",
            SftpTaskStatus::Done,
            0,
        );

        // 第 101 条终态产生：最旧的 terminal-0 被淘汰（Pending → Running → Done 合法迁移）
        assert!(service.transition_task(
            &app.handle(),
            "pending-new",
            "session-1",
            SftpTaskStatus::Running,
            None
        ));
        assert!(service.transition_task(
            &app.handle(),
            "pending-new",
            "session-1",
            SftpTaskStatus::Done,
            None
        ));

        let tasks = service.tasks.lock().unwrap();
        let terminal_count = tasks
            .values()
            .filter(|task| task.session_id == "session-1" && is_terminal(&task.status))
            .count();
        assert_eq!(terminal_count, 100, "每 Session 终态上限为 100");
        assert!(!tasks.contains_key("terminal-0"), "最旧终态任务应被淘汰");
        assert!(tasks.contains_key("terminal-1"));
        assert!(tasks.contains_key("pending-new"));
        assert!(tasks.contains_key("running-old"), "活动任务不得被淘汰");
        assert!(
            tasks.contains_key("other-terminal"),
            "其他 Session 的终态任务不得被本 Session 淘汰"
        );
    }

    /// 恰好 100 条终态时不做任何淘汰（边界不越界）。
    #[test]
    fn terminal_eviction_at_exact_100_keeps_all() {
        use tauri::test::mock_app;
        let app = mock_app();
        let service = make_service();
        service.register_session("session-1".to_string(), make_host());
        for i in 0..99 {
            insert_registry_task(
                &service,
                "session-1",
                &format!("terminal-{}", i),
                SftpTaskStatus::Done,
                i,
            );
        }
        insert_registry_task(
            &service,
            "session-1",
            "pending-new",
            SftpTaskStatus::Pending,
            1000,
        );

        assert!(service.transition_task(
            &app.handle(),
            "pending-new",
            "session-1",
            SftpTaskStatus::Running,
            None
        ));
        assert!(service.transition_task(
            &app.handle(),
            "pending-new",
            "session-1",
            SftpTaskStatus::Done,
            None
        ));

        let tasks = service.tasks.lock().unwrap();
        assert_eq!(
            tasks
                .values()
                .filter(|task| is_terminal(&task.status))
                .count(),
            100
        );
        assert!(
            tasks.contains_key("terminal-0"),
            "恰好 100 条终态时不得淘汰"
        );
    }

    /// 相同 createdAt 的终态任务按 task_id 升序淘汰最小者，结果跨运行确定。
    #[test]
    fn terminal_eviction_with_equal_created_at_is_deterministic() {
        use tauri::test::mock_app;
        let app = mock_app();
        let service = make_service();
        service.register_session("session-1".to_string(), make_host());
        for i in 0..100 {
            insert_registry_task(
                &service,
                "session-1",
                &format!("t-{:03}", i),
                SftpTaskStatus::Done,
                0,
            );
        }
        insert_registry_task(
            &service,
            "session-1",
            "pending-new",
            SftpTaskStatus::Pending,
            1,
        );

        assert!(service.transition_task(
            &app.handle(),
            "pending-new",
            "session-1",
            SftpTaskStatus::Running,
            None
        ));
        assert!(service.transition_task(
            &app.handle(),
            "pending-new",
            "session-1",
            SftpTaskStatus::Done,
            None
        ));

        let tasks = service.tasks.lock().unwrap();
        assert!(
            !tasks.contains_key("t-000"),
            "相同 createdAt 时按 task_id 升序淘汰最小者"
        );
        assert!(tasks.contains_key("t-099"));
        assert!(tasks.contains_key("pending-new"));
    }

    /// 清除终态只移除 Done/Failed/Cancelled；活动任务与其他 Session 保留，重复调用幂等。
    #[test]
    fn clear_terminal_tasks_removes_only_terminal_and_is_idempotent() {
        let service = make_service();
        service.register_session("session-1".to_string(), make_host());
        insert_registry_task(&service, "session-1", "done", SftpTaskStatus::Done, 1);
        insert_registry_task(&service, "session-1", "failed", SftpTaskStatus::Failed, 2);
        insert_registry_task(
            &service,
            "session-1",
            "cancelled",
            SftpTaskStatus::Cancelled,
            3,
        );
        insert_registry_task(&service, "session-1", "running", SftpTaskStatus::Running, 4);
        insert_registry_task(&service, "session-1", "pending", SftpTaskStatus::Pending, 5);
        insert_registry_task(&service, "session-2", "other-done", SftpTaskStatus::Done, 6);

        service.clear_terminal_tasks("session-1");

        let ids: Vec<String> = service.tasks.lock().unwrap().keys().cloned().collect();
        assert!(ids.contains(&"running".to_string()), "Running 必须保留");
        assert!(ids.contains(&"pending".to_string()), "Pending 必须保留");
        assert!(
            ids.contains(&"other-done".to_string()),
            "其他 Session 任务不受影响"
        );
        assert!(!ids.contains(&"done".to_string()));
        assert!(!ids.contains(&"failed".to_string()));
        assert!(!ids.contains(&"cancelled".to_string()));
        assert_eq!(ids.len(), 3);

        service.clear_terminal_tasks("session-1");
        assert_eq!(service.tasks.lock().unwrap().len(), 3, "重复清除应幂等");
    }

    /// Session 关闭保留活任务，迟到 worker 的唯一终态仍必须结案并移除任务。
    #[test]
    fn session_close_clears_registry_and_rejects_late_worker_updates() {
        use tauri::test::mock_app;
        let app = mock_app();
        let service = make_service();
        service.register_session("session-1".to_string(), make_host());
        let token = insert_task(&service, "task-late", SftpTaskStatus::Pending);

        service.cleanup_session("session-1", &app.handle().clone());

        assert!(token.is_cancelled());
        assert!(
            service.tasks.lock().unwrap().contains_key("task-late"),
            "关闭后活任务应保留到真实终态"
        );
        assert!(
            service.transition_task(
                &app.handle(),
                "task-late",
                "session-1",
                SftpTaskStatus::Cancelled,
                None
            ),
            "迟到 worker 必须发布最终 Cancelled"
        );
        assert!(service.tasks.lock().unwrap().is_empty());
    }

    // ─── 上传临时文件路径（发布目标目录）──────────────────────────────────

    /// 上传临时文件与最终目标同目录且命名包含 taskId：与下载命名规则一致，
    /// 全局唯一的 taskId 保证同目标并发任务不会撞名。
    #[test]
    fn upload_temp_path_lives_in_target_directory_with_task_id() {
        let temp_path = upload_temp_path("/srv/data/file.bin", "task-42");

        assert_eq!(temp_path, "/srv/data/.file.bin.task-42.part");
    }

    /// 根目录下的目标文件：临时文件仍在同一根目录，无重复斜杠。
    #[test]
    fn upload_temp_path_handles_root_directory() {
        let temp_path = upload_temp_path("/file.bin", "task-7");

        assert_eq!(temp_path, "/.file.bin.task-7.part");
    }

    // ─── 上传发布（publish_upload_file）contract ───────────────────────────

    /// 目标不存在 + Reject：no-clobber 重命名发布成功，临时路径消失、内容落位。
    #[test]
    fn publish_upload_new_target_renames_temp_into_place() {
        use crate::core::ssh_transport::test_support::{in_memory_sftp, in_memory_sftp_transport};

        let fs = in_memory_sftp(&[("/srv/.f.txt.task-1.part", b"new".to_vec())]);
        let mut transport = in_memory_sftp_transport(&fs);

        publish_upload_file(
            &mut transport,
            "/srv/.f.txt.task-1.part",
            "/srv/f.txt",
            ConflictStrategy::Reject,
        )
        .expect("新目标发布应成功");

        assert_eq!(fs.content("/srv/f.txt"), Some(b"new".to_vec()));
        assert!(
            !fs.has_file("/srv/.f.txt.task-1.part"),
            "发布后临时文件不应残留"
        );
    }

    /// 目标已存在 + Reject：返回 SftpTargetExists，旧目标内容与临时文件均不动。
    #[test]
    fn publish_upload_reject_keeps_existing_target() {
        use crate::core::ssh_transport::test_support::{in_memory_sftp, in_memory_sftp_transport};

        let fs = in_memory_sftp(&[
            ("/srv/f.txt", b"old".to_vec()),
            ("/srv/.f.txt.task-1.part", b"new".to_vec()),
        ]);
        let mut transport = in_memory_sftp_transport(&fs);

        let error = publish_upload_file(
            &mut transport,
            "/srv/.f.txt.task-1.part",
            "/srv/f.txt",
            ConflictStrategy::Reject,
        )
        .expect_err("目标已存在 + Reject 必须失败");

        assert!(
            matches!(&error, AppError::SftpTargetExists(path) if path.to_string() == "/srv/f.txt"),
            "应返回结构化 SftpTargetExists，实际: {error:?}"
        );
        assert_eq!(fs.content("/srv/f.txt"), Some(b"old".to_vec()));
        assert!(
            fs.has_file("/srv/.f.txt.task-1.part"),
            "发布拒绝不得删除临时文件（由 worker 统一清理）"
        );
    }

    /// 目标已存在 + Overwrite：远端原子替换，旧内容被新内容替换、临时路径消失。
    #[test]
    fn publish_upload_overwrite_atomically_replaces_target() {
        use crate::core::ssh_transport::test_support::{in_memory_sftp, in_memory_sftp_transport};

        let fs = in_memory_sftp(&[
            ("/srv/f.txt", b"old".to_vec()),
            ("/srv/.f.txt.task-1.part", b"new".to_vec()),
        ]);
        let mut transport = in_memory_sftp_transport(&fs);

        publish_upload_file(
            &mut transport,
            "/srv/.f.txt.task-1.part",
            "/srv/f.txt",
            ConflictStrategy::Overwrite,
        )
        .expect("支持原子替换的远端应发布成功");

        assert_eq!(fs.content("/srv/f.txt"), Some(b"new".to_vec()));
        assert!(!fs.has_file("/srv/.f.txt.task-1.part"));
    }

    /// 目标已存在 + Overwrite 且远端不支持原子替换：SftpPublishError，
    /// 旧目标内容保持不动，绝不先删旧文件。
    #[test]
    fn publish_upload_overwrite_fails_without_touching_target_when_unsupported() {
        use crate::core::ssh_transport::test_support::{
            in_memory_sftp_no_atomic_replace, in_memory_sftp_transport,
        };

        let fs = in_memory_sftp_no_atomic_replace(&[
            ("/srv/f.txt", b"old".to_vec()),
            ("/srv/.f.txt.task-1.part", b"new".to_vec()),
        ]);
        let mut transport = in_memory_sftp_transport(&fs);

        let error = publish_upload_file(
            &mut transport,
            "/srv/.f.txt.task-1.part",
            "/srv/f.txt",
            ConflictStrategy::Overwrite,
        )
        .expect_err("不支持原子替换时必须失败");

        assert!(
            matches!(&error, AppError::SftpPublishError(detail)
                if detail.to_string().contains("无法保证安全替换") && detail.to_string().contains("旧目标保留")),
            "应保留旧目标并给出结构化发布错误，实际: {error:?}"
        );
        assert_eq!(
            fs.content("/srv/f.txt"),
            Some(b"old".to_vec()),
            "发布失败不得改动旧目标"
        );
        assert!(fs.has_file("/srv/.f.txt.task-1.part"));
    }

    /// 目标检查的元数据错误（非路径不存在）原样传播，不做发布尝试。
    #[test]
    fn publish_upload_propagates_metadata_errors() {
        use crate::core::ssh_transport::test_support::failing_channel_sftp;

        let mut transport = failing_channel_sftp();

        let error = publish_upload_file(
            &mut transport,
            "/srv/.f.txt.task-1.part",
            "/srv/f.txt",
            ConflictStrategy::Reject,
        )
        .expect_err("元数据错误应传播");

        assert!(
            matches!(&error, AppError::SftpChannelError(message) if message.to_string().contains("connection lost")),
            "元数据错误应原样返回，实际: {error:?}"
        );
    }

    // ─── 上传安全发布与清理 worker contract ───────────────────────────────

    /// 新文件上传：数据经远端临时文件发布到最终目标，Done 后目标内容正确、
    /// 临时文件不残留，发布成功不触发任何 unlink。
    #[tokio::test(flavor = "multi_thread")]
    async fn upload_new_file_publishes_to_target_without_temp_leftover() {
        use crate::core::ssh_transport::test_support::{in_memory_sftp, in_memory_sftp_transport};
        use tauri::test::mock_app;

        let app = mock_app();
        let fs = in_memory_sftp(&[]);
        let fs_for_connector = fs.clone();
        let service = SftpService::with_connector(move |_, _| {
            Ok(in_memory_sftp_transport(&fs_for_connector))
        });
        service.register_session("session-1".to_string(), make_host());

        let local_path =
            std::env::temp_dir().join(format!("titan-upload-new-{}.bin", Uuid::new_v4()));
        std::fs::write(&local_path, b"hello").unwrap();
        let task = service
            .enqueue_upload(
                "session-1".to_string(),
                local_path.to_string_lossy().to_string(),
                "/srv".to_string(),
                ConflictStrategy::Reject,
                app.handle().clone(),
            )
            .expect("新文件上传应正常入队");

        assert_eq!(
            wait_for_terminal(&service, &task.task_id),
            SftpTaskStatus::Done
        );
        assert_eq!(
            fs.content(&task.remote_path),
            Some(b"hello".to_vec()),
            "目标内容应为本地文件内容"
        );
        let temp_path = upload_temp_path(&task.remote_path, &task.task_id);
        assert!(!fs.has_file(&temp_path), "发布成功后临时文件不应残留");
        assert!(fs.unlink_calls().is_empty(), "发布成功不得触发任何 unlink");
        let _ = std::fs::remove_file(&local_path);
    }

    /// 零字节文件上传：空内容仍经完整临时文件流程发布，目标以空文件存在。
    #[tokio::test(flavor = "multi_thread")]
    async fn upload_zero_byte_file_publishes_empty_target() {
        use crate::core::ssh_transport::test_support::{in_memory_sftp, in_memory_sftp_transport};
        use tauri::test::mock_app;

        let app = mock_app();
        let fs = in_memory_sftp(&[]);
        let fs_for_connector = fs.clone();
        let service = SftpService::with_connector(move |_, _| {
            Ok(in_memory_sftp_transport(&fs_for_connector))
        });
        service.register_session("session-1".to_string(), make_host());

        let local_path =
            std::env::temp_dir().join(format!("titan-upload-zero-{}.bin", Uuid::new_v4()));
        std::fs::write(&local_path, b"").unwrap();
        let task = service
            .enqueue_upload(
                "session-1".to_string(),
                local_path.to_string_lossy().to_string(),
                "/srv".to_string(),
                ConflictStrategy::Reject,
                app.handle().clone(),
            )
            .expect("零字节文件上传应正常入队");

        assert_eq!(
            wait_for_terminal(&service, &task.task_id),
            SftpTaskStatus::Done
        );
        assert_eq!(
            fs.content(&task.remote_path),
            Some(Vec::new()),
            "零字节文件应以空内容发布到目标"
        );
        let temp_path = upload_temp_path(&task.remote_path, &task.task_id);
        assert!(!fs.has_file(&temp_path));
        let _ = std::fs::remove_file(&local_path);
    }

    /// Reject + 远端目标已存在：任务 Failed + SftpTargetExists，旧远端内容不动，
    /// 本任务临时文件被清理，未知 .part 文件不被扫描或删除。
    #[tokio::test(flavor = "multi_thread")]
    async fn upload_reject_keeps_remote_target_and_cleans_only_own_temp() {
        use crate::core::ssh_transport::test_support::{in_memory_sftp, in_memory_sftp_transport};
        use tauri::test::mock_app;

        let app = mock_app();
        let fs = in_memory_sftp(&[
            ("/srv/keep.txt", b"old".to_vec()),
            ("/srv/.other.unknown.part", b"x".to_vec()),
        ]);
        let fs_for_connector = fs.clone();
        let service = SftpService::with_connector(move |_, _| {
            Ok(in_memory_sftp_transport(&fs_for_connector))
        });
        service.register_session("session-1".to_string(), make_host());

        // 独立子目录承载同名本地文件：远端目标 /srv/keep.txt 与预置目标冲突
        let local_dir =
            std::env::temp_dir().join(format!("titan-upload-reject-{}", Uuid::new_v4()));
        std::fs::create_dir(&local_dir).unwrap();
        let local_path = local_dir.join("keep.txt");
        std::fs::write(&local_path, b"new").unwrap();
        let task = service
            .enqueue_upload(
                "session-1".to_string(),
                local_path.to_string_lossy().to_string(),
                "/srv".to_string(),
                ConflictStrategy::Reject,
                app.handle().clone(),
            )
            .expect("上传应正常入队");
        let temp_path = upload_temp_path(&task.remote_path, &task.task_id);

        assert_eq!(
            wait_for_terminal(&service, &task.task_id),
            SftpTaskStatus::Failed
        );
        let registry_task = service
            .tasks
            .lock()
            .unwrap()
            .get(&task.task_id)
            .unwrap()
            .clone();
        let error = registry_task
            .error
            .as_ref()
            .expect("拒绝覆盖必须携带结构化错误");
        assert_eq!(error.code, "SftpTargetExists");
        assert!(
            error
                .detail
                .as_deref()
                .is_some_and(|detail| detail.contains("/srv/keep.txt")),
            "错误 detail 必须包含远端目标路径"
        );
        assert_eq!(
            fs.content("/srv/keep.txt"),
            Some(b"old".to_vec()),
            "Reject 冲突不得破坏远端旧目标"
        );
        assert!(!fs.has_file(&temp_path), "冲突失败后本任务临时文件应被清理");
        assert_eq!(
            fs.unlink_calls(),
            vec![temp_path.clone()],
            "只清理本任务临时文件，不扫描未知 .part"
        );
        assert!(
            fs.has_file("/srv/.other.unknown.part"),
            "未知 .part 文件不得被删除"
        );
        let _ = std::fs::remove_dir_all(&local_dir);
    }

    /// 确认覆盖：Overwrite 上传原子替换远端目标，任务 Done，目标内容为新内容。
    #[tokio::test(flavor = "multi_thread")]
    async fn upload_overwrite_replaces_remote_target() {
        use crate::core::ssh_transport::test_support::{in_memory_sftp, in_memory_sftp_transport};
        use tauri::test::mock_app;

        let app = mock_app();
        let fs = in_memory_sftp(&[("/srv/keep.txt", b"old".to_vec())]);
        let fs_for_connector = fs.clone();
        let service = SftpService::with_connector(move |_, _| {
            Ok(in_memory_sftp_transport(&fs_for_connector))
        });
        service.register_session("session-1".to_string(), make_host());

        let local_dir =
            std::env::temp_dir().join(format!("titan-upload-overwrite-{}", Uuid::new_v4()));
        std::fs::create_dir(&local_dir).unwrap();
        let local_path = local_dir.join("keep.txt");
        std::fs::write(&local_path, b"new").unwrap();
        let task = service
            .enqueue_upload(
                "session-1".to_string(),
                local_path.to_string_lossy().to_string(),
                "/srv".to_string(),
                ConflictStrategy::Overwrite,
                app.handle().clone(),
            )
            .expect("确认覆盖的上传应正常入队");

        assert_eq!(
            wait_for_terminal(&service, &task.task_id),
            SftpTaskStatus::Done
        );
        assert_eq!(
            fs.content("/srv/keep.txt"),
            Some(b"new".to_vec()),
            "确认覆盖后目标内容应为新内容"
        );
        let temp_path = upload_temp_path(&task.remote_path, &task.task_id);
        assert!(!fs.has_file(&temp_path));
        assert!(fs.unlink_calls().is_empty(), "原子替换不得触发 unlink");
        let _ = std::fs::remove_dir_all(&local_dir);
    }

    /// 远端不支持原子替换 + 目标已存在 + Overwrite：任务 Failed + SftpPublishError，
    /// 旧目标内容保持不动，临时文件被清理。
    #[tokio::test(flavor = "multi_thread")]
    async fn upload_overwrite_fails_when_server_cannot_replace_atomically() {
        use crate::core::ssh_transport::test_support::{
            in_memory_sftp_no_atomic_replace, in_memory_sftp_transport,
        };
        use tauri::test::mock_app;

        let app = mock_app();
        let fs = in_memory_sftp_no_atomic_replace(&[("/srv/keep.txt", b"old".to_vec())]);
        let fs_for_connector = fs.clone();
        let service = SftpService::with_connector(move |_, _| {
            Ok(in_memory_sftp_transport(&fs_for_connector))
        });
        service.register_session("session-1".to_string(), make_host());

        let local_dir =
            std::env::temp_dir().join(format!("titan-upload-noatomic-{}", Uuid::new_v4()));
        std::fs::create_dir(&local_dir).unwrap();
        let local_path = local_dir.join("keep.txt");
        std::fs::write(&local_path, b"new").unwrap();
        let task = service
            .enqueue_upload(
                "session-1".to_string(),
                local_path.to_string_lossy().to_string(),
                "/srv".to_string(),
                ConflictStrategy::Overwrite,
                app.handle().clone(),
            )
            .expect("上传应正常入队");
        let temp_path = upload_temp_path(&task.remote_path, &task.task_id);

        assert_eq!(
            wait_for_terminal(&service, &task.task_id),
            SftpTaskStatus::Failed
        );
        let registry_task = service
            .tasks
            .lock()
            .unwrap()
            .get(&task.task_id)
            .unwrap()
            .clone();
        let error = registry_task
            .error
            .as_ref()
            .expect("发布失败必须携带结构化错误");
        assert_eq!(error.code, "SftpPublishError");
        let detail_key = error.detail_key.as_deref().unwrap_or_default();
        assert!(
            detail_key.contains("无法保证安全替换") && detail_key.contains("旧目标保留"),
            "发布失败必须说明旧目标保留，实际: {detail_key}"
        );
        assert_eq!(
            fs.content("/srv/keep.txt"),
            Some(b"old".to_vec()),
            "发布失败不得改动远端旧目标"
        );
        assert!(!fs.has_file(&temp_path), "临时文件应被清理");
        let _ = std::fs::remove_dir_all(&local_dir);
    }

    /// 运行时写入失败：任务 Failed + SftpWriteError，本任务临时文件被清理，
    /// 远端旧目标保持不动。
    #[tokio::test(flavor = "multi_thread")]
    async fn upload_write_failure_cleans_temp_and_keeps_target() {
        use crate::core::ssh_transport::test_support::{
            Gate, gated_in_memory_sftp, in_memory_sftp_transport,
        };
        use tauri::test::mock_app;

        let app = mock_app();
        let gate = Gate::new();
        let fs = gated_in_memory_sftp(&[("/srv/keep.txt", b"old".to_vec())], gate.clone(), true);
        let fs_for_connector = fs.clone();
        let service = SftpService::with_connector(move |_, _| {
            Ok(in_memory_sftp_transport(&fs_for_connector))
        });
        service.register_session("session-1".to_string(), make_host());

        let local_dir =
            std::env::temp_dir().join(format!("titan-upload-writefail-{}", Uuid::new_v4()));
        std::fs::create_dir(&local_dir).unwrap();
        let local_path = local_dir.join("keep.txt");
        std::fs::write(&local_path, b"new").unwrap();
        let task = service
            .enqueue_upload(
                "session-1".to_string(),
                local_path.to_string_lossy().to_string(),
                "/srv".to_string(),
                ConflictStrategy::Reject,
                app.handle().clone(),
            )
            .expect("上传应正常入队");
        let temp_path = upload_temp_path(&task.remote_path, &task.task_id);
        gate.wait_arrived(); // 传输已创建临时文件并进入首次写入
        gate.open();

        assert_eq!(
            wait_for_terminal(&service, &task.task_id),
            SftpTaskStatus::Failed
        );
        let registry_task = service
            .tasks
            .lock()
            .unwrap()
            .get(&task.task_id)
            .unwrap()
            .clone();
        let error = registry_task
            .error
            .as_ref()
            .expect("写入失败必须携带结构化错误");
        assert_eq!(error.code, "SftpWriteError");
        assert!(!fs.has_file(&temp_path), "写入失败后本任务临时文件应被清理");
        assert_eq!(
            fs.unlink_calls(),
            vec![temp_path.clone()],
            "只清理本任务临时文件"
        );
        assert_eq!(fs.content("/srv/keep.txt"), Some(b"old".to_vec()));
        let _ = std::fs::remove_dir_all(&local_dir);
    }

    /// 传输中取消：任务 Cancelled 且无错误，本任务临时文件被清理，
    /// 远端旧目标保持不动。
    #[tokio::test(flavor = "multi_thread")]
    async fn upload_cancel_cleans_temp_and_keeps_target() {
        use crate::core::ssh_transport::test_support::{
            Gate, gated_in_memory_sftp, in_memory_sftp_transport,
        };
        use tauri::test::mock_app;

        let app = mock_app();
        let gate = Gate::new();
        let fs = gated_in_memory_sftp(&[("/srv/keep.txt", b"old".to_vec())], gate.clone(), false);
        let fs_for_connector = fs.clone();
        let service = SftpService::with_connector(move |_, _| {
            Ok(in_memory_sftp_transport(&fs_for_connector))
        });
        service.register_session("session-1".to_string(), make_host());

        let local_dir =
            std::env::temp_dir().join(format!("titan-upload-cancel-{}", Uuid::new_v4()));
        std::fs::create_dir(&local_dir).unwrap();
        let local_path = local_dir.join("keep.txt");
        std::fs::write(&local_path, b"new").unwrap();
        let task = service
            .enqueue_upload(
                "session-1".to_string(),
                local_path.to_string_lossy().to_string(),
                "/srv".to_string(),
                ConflictStrategy::Reject,
                app.handle().clone(),
            )
            .expect("上传应正常入队");
        let temp_path = upload_temp_path(&task.remote_path, &task.task_id);
        gate.wait_arrived();
        service.cancel_task(&task.task_id).unwrap();
        gate.open();

        assert_eq!(
            wait_for_terminal(&service, &task.task_id),
            SftpTaskStatus::Cancelled
        );
        let registry_task = service
            .tasks
            .lock()
            .unwrap()
            .get(&task.task_id)
            .unwrap()
            .clone();
        assert!(registry_task.error.is_none(), "清理成功的取消不得携带错误");
        assert!(!fs.has_file(&temp_path), "取消后本任务临时文件应被清理");
        assert_eq!(fs.content("/srv/keep.txt"), Some(b"old".to_vec()));
        let _ = std::fs::remove_dir_all(&local_dir);
    }

    /// 取消后清理失败：任务 Cancelled 且 error 报告包含临时路径的清理错误，
    /// 残留临时文件保留待用户处理。
    #[tokio::test(flavor = "multi_thread")]
    async fn upload_cancel_cleanup_failure_reports_temp_path() {
        use crate::core::ssh_transport::test_support::{
            Gate, gated_in_memory_sftp, in_memory_sftp_transport,
        };
        use tauri::test::mock_app;

        let app = mock_app();
        let gate = Gate::new();
        let fs = gated_in_memory_sftp(&[], gate.clone(), false);
        fs.deny_unlink();
        let fs_for_connector = fs.clone();
        let service = SftpService::with_connector(move |_, _| {
            Ok(in_memory_sftp_transport(&fs_for_connector))
        });
        service.register_session("session-1".to_string(), make_host());

        let local_path =
            std::env::temp_dir().join(format!("titan-upload-cleanfail-{}.bin", Uuid::new_v4()));
        std::fs::write(&local_path, b"new").unwrap();
        let task = service
            .enqueue_upload(
                "session-1".to_string(),
                local_path.to_string_lossy().to_string(),
                "/srv".to_string(),
                ConflictStrategy::Reject,
                app.handle().clone(),
            )
            .expect("上传应正常入队");
        let temp_path = upload_temp_path(&task.remote_path, &task.task_id);
        let temp_str = temp_path.clone();
        gate.wait_arrived();
        service.cancel_task(&task.task_id).unwrap();
        gate.open();

        assert_eq!(
            wait_for_terminal(&service, &task.task_id),
            SftpTaskStatus::Cancelled
        );
        let registry_task = service
            .tasks
            .lock()
            .unwrap()
            .get(&task.task_id)
            .unwrap()
            .clone();
        let error = registry_task.error.as_ref().expect("清理失败必须报告错误");
        assert_eq!(error.code, "SftpTransferError");
        let detail_key = error.detail_key.as_deref().unwrap_or_default();
        let params = error.detail_params.clone().unwrap_or_default();
        assert!(
            detail_key.contains("清理临时文件失败")
                && params.iter().any(|param| param.contains(&temp_str)),
            "清理失败错误必须包含临时路径，实际: {:?}",
            error
        );
        assert!(
            fs.has_file(&temp_str),
            "清理失败后残留临时文件保留，路径已报告给用户"
        );
        let _ = std::fs::remove_file(&local_path);
    }

    /// 写入失败叠加清理失败：任务 Failed 保留 SftpWriteError，
    /// detail 追加包含临时路径的清理诊断。
    #[tokio::test(flavor = "multi_thread")]
    async fn upload_failure_with_cleanup_failure_appends_temp_path() {
        use crate::core::ssh_transport::test_support::{
            Gate, gated_in_memory_sftp, in_memory_sftp_transport,
        };
        use tauri::test::mock_app;

        let app = mock_app();
        let gate = Gate::new();
        let fs = gated_in_memory_sftp(&[], gate.clone(), true);
        fs.deny_unlink();
        let fs_for_connector = fs.clone();
        let service = SftpService::with_connector(move |_, _| {
            Ok(in_memory_sftp_transport(&fs_for_connector))
        });
        service.register_session("session-1".to_string(), make_host());

        let local_path =
            std::env::temp_dir().join(format!("titan-upload-cleanfail2-{}.bin", Uuid::new_v4()));
        std::fs::write(&local_path, b"new").unwrap();
        let task = service
            .enqueue_upload(
                "session-1".to_string(),
                local_path.to_string_lossy().to_string(),
                "/srv".to_string(),
                ConflictStrategy::Reject,
                app.handle().clone(),
            )
            .expect("上传应正常入队");
        let temp_path = upload_temp_path(&task.remote_path, &task.task_id);
        let temp_str = temp_path.clone();
        gate.wait_arrived();
        gate.open();

        assert_eq!(
            wait_for_terminal(&service, &task.task_id),
            SftpTaskStatus::Failed
        );
        let registry_task = service
            .tasks
            .lock()
            .unwrap()
            .get(&task.task_id)
            .unwrap()
            .clone();
        let error = registry_task
            .error
            .as_ref()
            .expect("失败必须携带结构化错误");
        assert_eq!(
            error.code, "SftpWriteError",
            "主错误代码必须保留 SftpWriteError"
        );
        assert!(
            error
                .detail
                .as_deref()
                .is_some_and(|detail| detail.contains("remote write reset")
                    && detail.contains("清理临时文件失败")
                    && detail.contains(&temp_str)),
            "detail 必须同时保留写入诊断与临时路径清理诊断，实际: {:?}",
            error.detail
        );
        let _ = std::fs::remove_file(&local_path);
    }

    /// 同一 Session 已有 Pending/Running 上传占用相同最终目标时，后加入任务
    /// 被拒绝并返回结构化 SftpTargetBusy；原任务不受影响，终态后可再次上传。
    #[tokio::test(flavor = "multi_thread")]
    async fn enqueue_upload_duplicate_active_target_is_rejected() {
        use crate::core::ssh_transport::test_support::{
            Gate, gated_in_memory_sftp, in_memory_sftp_transport,
        };
        use tauri::test::mock_app;

        let app = mock_app();
        let gate = Gate::new();
        let fs = gated_in_memory_sftp(&[], gate.clone(), false);
        let fs_for_connector = fs.clone();
        let service = SftpService::with_connector(move |_, _| {
            Ok(in_memory_sftp_transport(&fs_for_connector))
        });
        service.register_session("session-1".to_string(), make_host());

        let local_dir = std::env::temp_dir().join(format!("titan-upload-dup-{}", Uuid::new_v4()));
        std::fs::create_dir(&local_dir).unwrap();
        let local_path = local_dir.join("keep.txt");
        std::fs::write(&local_path, b"new").unwrap();
        let first = service
            .enqueue_upload(
                "session-1".to_string(),
                local_path.to_string_lossy().to_string(),
                "/srv".to_string(),
                ConflictStrategy::Reject,
                app.handle().clone(),
            )
            .expect("首个上传应正常入队");
        gate.wait_arrived(); // 首个任务处于 Running 且占用 /srv/keep.txt

        let error = service
            .enqueue_upload(
                "session-1".to_string(),
                local_path.to_string_lossy().to_string(),
                "/srv".to_string(),
                ConflictStrategy::Reject,
                app.handle().clone(),
            )
            .expect_err("相同最终目标的第二个上传应被拒绝");
        assert!(
            matches!(&error, AppError::SftpTargetBusy(path) if path.to_string() == "/srv/keep.txt"),
            "重复目标应返回结构化 SftpTargetBusy，实际: {error:?}"
        );

        gate.open();
        assert_eq!(
            wait_for_terminal(&service, &first.task_id),
            SftpTaskStatus::Done,
            "原任务不受拒绝影响"
        );

        // 终态任务不再占用目标：再次上传相同目标应正常入队并完成
        let again = service
            .enqueue_upload(
                "session-1".to_string(),
                local_path.to_string_lossy().to_string(),
                "/srv".to_string(),
                ConflictStrategy::Overwrite,
                app.handle().clone(),
            )
            .expect("终态任务不占用目标，再次上传应成功入队");
        assert_eq!(
            wait_for_terminal(&service, &again.task_id),
            SftpTaskStatus::Done
        );
        let _ = std::fs::remove_dir_all(&local_dir);
    }

    /// 上传与下载各自占用命名空间：同 Session 的下载任务不阻止上传入队。
    #[tokio::test(flavor = "multi_thread")]
    async fn enqueue_upload_does_not_conflict_with_active_download() {
        use crate::core::ssh_transport::test_support::{in_memory_sftp, in_memory_sftp_transport};
        use tauri::test::mock_app;

        let app = mock_app();
        let fs = in_memory_sftp(&[("/remote/file.bin", b"content".to_vec())]);
        let fs_for_connector = fs.clone();
        let service = SftpService::with_connector(move |_, _| {
            Ok(in_memory_sftp_transport(&fs_for_connector))
        });
        service.register_session("session-1".to_string(), make_host());

        // 入队一个下载（立刻完成），再上传同名远端目标：两者目标命名空间不同
        let download_local =
            std::env::temp_dir().join(format!("titan-cross-{}.bin", Uuid::new_v4()));
        let download = service
            .enqueue_download(
                "session-1".to_string(),
                "/remote/file.bin".to_string(),
                download_local.to_string_lossy().to_string(),
                ConflictStrategy::Reject,
                app.handle().clone(),
            )
            .expect("下载应正常入队");

        let upload_local =
            std::env::temp_dir().join(format!("titan-cross-up-{}.bin", Uuid::new_v4()));
        std::fs::write(&upload_local, b"up").unwrap();
        let upload = service
            .enqueue_upload(
                "session-1".to_string(),
                upload_local.to_string_lossy().to_string(),
                "/remote".to_string(),
                ConflictStrategy::Reject,
                app.handle().clone(),
            )
            .expect("上传不得被下载任务阻止");

        assert_eq!(
            wait_for_terminal(&service, &download.task_id),
            SftpTaskStatus::Done
        );
        assert_eq!(
            wait_for_terminal(&service, &upload.task_id),
            SftpTaskStatus::Done
        );
        let _ = std::fs::remove_file(&download_local);
        let _ = std::fs::remove_file(&upload_local);
    }
}
