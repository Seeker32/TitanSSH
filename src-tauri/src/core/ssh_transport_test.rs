#[cfg(test)]
mod tests {
    use crate::core::shared_exec_registry::ExecConnectionEntry;
    use crate::core::ssh_transport::test_support::allow_all_verifier;
    use crate::core::ssh_transport::{
        ConnectPhase, LIBSSH2_ERROR_EAGAIN, RemoteFile, SSH_PROTOCOL_TIMEOUT_MS, SftpEntry,
        SftpOps, SftpTransport, TerminalOps, TerminalTransport, build_connect_error,
        clear_long_lived_session_timeout, connect_tcp_stream, decode_exec_output,
        map_sftp_path_error, map_sftp_rename_error, resolve_socket_addrs,
        resolve_socket_addrs_with_timeout, socket_endpoint,
    };
    use crate::errors::app_error::AppError;
    use crate::models::host::{AuthType, HostConfig};
    use std::collections::VecDeque;
    use std::io::{self, Write};
    use std::net::{SocketAddr, ToSocketAddrs};
    use std::sync::{Arc, Barrier, Mutex};
    use std::time::{Duration, Instant};

    struct RecordingTerminal {
        writes: Arc<Mutex<Vec<String>>>,
    }

    struct BlockingSftp {
        started: Arc<Barrier>,
        release: Arc<Barrier>,
    }

    impl SftpOps for BlockingSftp {
        /// 阻塞 SFTP 操作，验证它不会占用 Terminal capability。
        fn list_dir(&mut self, _path: &str) -> Result<Vec<SftpEntry>, AppError> {
            self.started.wait();
            self.release.wait();
            Ok(Vec::new())
        }

        /// 本测试不查询文件大小。
        fn file_size(&mut self, _path: &str) -> Result<u64, AppError> {
            Ok(0)
        }

        /// 本测试不打开文件。
        fn open_read(&mut self, _path: &str) -> Result<RemoteFile, AppError> {
            Err(AppError::SftpTransferError("unused".to_string().into()))
        }

        /// 本测试不创建文件。
        fn create(&mut self, _path: &str) -> Result<RemoteFile, AppError> {
            Err(AppError::SftpTransferError("unused".to_string().into()))
        }

        /// 本测试无需删除文件。
        fn unlink(&mut self, _path: &str) -> Result<(), AppError> {
            Ok(())
        }

        /// 本 adapter 不提供远端重命名。
        fn rename(&mut self, _src: &str, _dst: &str, _overwrite: bool) -> Result<(), AppError> {
            Err(AppError::SftpTransferError("unused".to_string().into()))
        }
    }

    impl TerminalOps for RecordingTerminal {
        /// 测试 adapter 不产生远端输出。
        fn read(&mut self, _buffer: &mut [u8]) -> io::Result<usize> {
            Ok(0)
        }

        /// 测试 adapter 记录写入内容。
        fn write(&mut self, data: &str) -> Result<(), AppError> {
            self.writes.lock().unwrap().push(data.to_string());
            Ok(())
        }

        /// 测试 adapter 接受任意终端尺寸。
        fn resize(&mut self, _cols: u32, _rows: u32) -> Result<(), AppError> {
            Ok(())
        }

        /// 测试 adapter 始终保持打开状态。
        fn eof(&self) -> bool {
            false
        }

        /// 测试 adapter 可无错误关闭。
        fn close(&mut self) -> Result<(), AppError> {
            Ok(())
        }
    }

    /// Terminal capability 只暴露行为，并将实现细节留在 transport module 内。
    #[test]
    fn terminal_capability_delegates_without_exposing_ssh2() {
        let writes = Arc::new(Mutex::new(Vec::new()));
        let mut terminal = TerminalTransport::from_backend(RecordingTerminal {
            writes: writes.clone(),
        });

        terminal.write("uptime\n").unwrap();

        assert_eq!(*writes.lock().unwrap(), vec!["uptime\n"]);
    }

    /// 非阻塞终端在 SSH window 暂满时必须重试，并在发生部分写入后继续发送余下
    /// 输入；flush 的 EAGAIN 也不得让已接受的粘贴内容被丢弃。
    #[test]
    fn terminal_input_retries_would_block_and_preserves_partial_progress() {
        struct BackpressuredWriter {
            write_results: VecDeque<io::Result<usize>>,
            flush_results: VecDeque<io::Result<()>>,
            written: Vec<u8>,
        }

        impl Write for BackpressuredWriter {
            /// 按预设结果模拟 SSH channel 写入，成功时复制对应字节。
            fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
                match self.write_results.pop_front().unwrap_or(Ok(buffer.len())) {
                    Ok(size) => {
                        self.written.extend_from_slice(&buffer[..size]);
                        Ok(size)
                    }
                    Err(error) => Err(error),
                }
            }

            /// 按预设结果模拟 SSH channel flush。
            fn flush(&mut self) -> io::Result<()> {
                self.flush_results.pop_front().unwrap_or(Ok(()))
            }
        }

        let mut writer = BackpressuredWriter {
            write_results: VecDeque::from([
                Err(io::Error::new(io::ErrorKind::WouldBlock, "window full")),
                Ok(2),
                Err(io::Error::new(io::ErrorKind::WouldBlock, "window full")),
                Ok(3),
            ]),
            flush_results: VecDeque::from([
                Err(io::Error::new(io::ErrorKind::WouldBlock, "window full")),
                Ok(()),
            ]),
            written: Vec::new(),
        };

        crate::core::ssh_transport::write_terminal_input_with_retry(
            &mut writer,
            b"paste",
            Instant::now() + Duration::from_secs(1),
            Duration::ZERO,
        )
        .expect("短暂背压恢复后，完整终端输入应发送成功");

        assert_eq!(writer.written, b"paste");
        assert!(
            writer.flush_results.is_empty(),
            "flush 的 EAGAIN 必须被重试"
        );
    }

    /// 若背压持续到已过的截止时间，终端输入必须有界返回，而不能在工作线程中忙等。
    #[test]
    fn terminal_input_returns_would_block_after_backpressure_deadline() {
        struct AlwaysBlockedWriter;

        impl Write for AlwaysBlockedWriter {
            /// 模拟始终已满的 SSH channel window。
            fn write(&mut self, _buffer: &[u8]) -> io::Result<usize> {
                Err(io::Error::new(io::ErrorKind::WouldBlock, "window full"))
            }

            /// 本测试不会到达 flush。
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }

        let error = crate::core::ssh_transport::write_terminal_input_with_retry(
            &mut AlwaysBlockedWriter,
            b"paste",
            Instant::now(),
            Duration::ZERO,
        )
        .expect_err("持续背压超过截止时间必须返回");

        assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
    }

    /// 调整 PTY 尺寸和关闭 channel 同样是非阻塞请求；一次 EAGAIN 不得被误报为
    /// 协议失败，而应在背压恢复后完成。
    #[test]
    fn terminal_control_requests_retry_eagain() {
        let mut resize_attempts = 0;
        crate::core::ssh_transport::retry_terminal_protocol_operation_with_retry(
            || {
                resize_attempts += 1;
                if resize_attempts == 1 {
                    Err(ssh2::Error::new(
                        ssh2::ErrorCode::Session(LIBSSH2_ERROR_EAGAIN),
                        "window full",
                    ))
                } else {
                    Ok(())
                }
            },
            Instant::now() + Duration::from_secs(1),
            Duration::ZERO,
        )
        .expect("resize 的短暂 EAGAIN 应重试成功");

        let mut close_attempts = 0;
        crate::core::ssh_transport::retry_terminal_protocol_operation_with_retry(
            || {
                close_attempts += 1;
                if close_attempts == 1 {
                    Err(ssh2::Error::new(
                        ssh2::ErrorCode::Session(LIBSSH2_ERROR_EAGAIN),
                        "window full",
                    ))
                } else {
                    Ok(())
                }
            },
            Instant::now() + Duration::from_secs(1),
            Duration::ZERO,
        )
        .expect("close 的短暂 EAGAIN 应重试成功");

        assert_eq!(resize_attempts, 2);
        assert_eq!(close_attempts, 2);

        let mut failure_attempts = 0;
        let error = crate::core::ssh_transport::retry_terminal_protocol_operation_with_retry(
            || {
                failure_attempts += 1;
                Err::<(), _>(ssh2::Error::new(
                    ssh2::ErrorCode::Session(-7), // LIBSSH2_ERROR_SOCKET_SEND
                    "socket failed",
                ))
            },
            Instant::now() + Duration::from_secs(1),
            Duration::ZERO,
        )
        .expect_err("非 EAGAIN 协议错误不得被重试或掩盖");

        assert_eq!(failure_attempts, 1);
        assert_eq!(error.code(), ssh2::ErrorCode::Session(-7));
    }

    /// 阻塞的 SFTP adapter 不得阻塞独立 Terminal capability。
    #[test]
    fn blocking_sftp_does_not_block_terminal_capability() {
        let started = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let mut sftp = SftpTransport::from_backend(BlockingSftp {
            started: started.clone(),
            release: release.clone(),
        });
        let writes = Arc::new(Mutex::new(Vec::new()));
        let mut terminal = TerminalTransport::from_backend(RecordingTerminal {
            writes: writes.clone(),
        });

        let transfer = std::thread::spawn(move || sftp.list_dir("/"));
        started.wait();
        let before = Instant::now();
        terminal.write("echo responsive\n").unwrap();
        let elapsed = before.elapsed();
        release.wait();

        assert!(elapsed < Duration::from_millis(100));
        assert_eq!(*writes.lock().unwrap(), vec!["echo responsive\n"]);
        assert!(transfer.join().unwrap().is_ok());
    }

    /// 构造密码认证测试主机。
    fn make_host(host: &str, port: u16) -> HostConfig {
        HostConfig {
            id: "host-1".to_string(),
            name: "test".to_string(),
            host: host.to_string(),
            port,
            username: "root".to_string(),
            auth_type: AuthType::Password,
            password_ref: Some("ref".to_string()),
            private_key_path: None,
            passphrase_ref: None,
            remark: None,
            group: String::new(),
        }
    }

    /// 非法主机名必须返回解析错误。
    #[test]
    fn resolve_socket_addrs_returns_error_for_invalid_host() {
        assert!(resolve_socket_addrs(&make_host("invalid host name with spaces", 22)).is_err());
    }

    /// 裸 IPv6 字面量必须使用方括号与端口组成可解析的 socket endpoint。
    #[test]
    fn socket_endpoint_brackets_bare_ipv6_without_changing_other_hosts() {
        let ipv6_endpoint = socket_endpoint("fe80::1", 22);
        assert_eq!(ipv6_endpoint, "[fe80::1]:22");
        assert!(
            ipv6_endpoint.to_socket_addrs().is_ok(),
            "加方括号后的 IPv6 endpoint 必须可由 ToSocketAddrs 解析"
        );
        assert_eq!(socket_endpoint("192.0.2.10", 2200), "192.0.2.10:2200");
        assert_eq!(
            socket_endpoint("ssh.example.test", 22),
            "ssh.example.test:22"
        );
    }

    /// 非超时网络错误保持连接失败语义。
    #[test]
    fn connect_tcp_stream_returns_connection_error_without_timeout() {
        let address: SocketAddr = "127.0.0.1:1".parse().unwrap();
        let result = connect_tcp_stream(&[address], Duration::from_millis(50));

        assert!(matches!(
            result,
            Err(AppError::SshConnectionError(message)) if message.to_string().contains("连接失败")
        ));
    }

    /// 多地址建连失败必须保留总尝试次数与最后错误；一次早期超时不得覆盖随后
    /// 的 ConnectionRefused 并伪装成纯超时。
    #[test]
    fn build_connect_error_reports_attempt_count_and_last_error() {
        let error = build_connect_error(
            2,
            Some(io::Error::new(io::ErrorKind::ConnectionRefused, "refused")),
            Duration::from_secs(10),
        );

        assert!(matches!(
            error,
            AppError::SshConnectionError(message)
                if message.to_string().contains("2")
                    && message.to_string().contains("refused")
                    && !message.to_string().contains("Connection timeout")
        ));
    }

    /// DNS 解析同样属于总连接预算；阻塞解析不得使连接等待超过给定 deadline。
    ///
    /// 用 rendezvous 栅栏做决定性验证（不依赖墙钟，避免 CI 负载导致的计时抖动）：
    /// 解析线程阻塞在栅栏上。若调用正确地按 deadline 返回，则调用返回时解析线程
    /// 必然仍阻塞在栅栏上（try_send 成功）；若超时机制回归（如误用 recv 等待解析），
    /// 调用会等解析线程完成，栅栏已无人等待（try_send 失败）。
    #[test]
    fn dns_resolution_returns_when_connect_deadline_expires() {
        let (release_tx, release_rx) = std::sync::mpsc::sync_channel(0);
        let started = Instant::now();
        let error = resolve_socket_addrs_with_timeout(
            "slow.example:22".to_string(),
            started + Duration::from_millis(20),
            move || {
                // 模拟慢 DNS：阻塞在栅栏上；兜底 5s，防止回归时测试无限挂起。
                let _ = release_rx.recv_timeout(Duration::from_secs(5));
                Ok(Vec::new())
            },
        )
        .expect_err("阻塞 DNS 解析必须在连接 deadline 返回");

        // 解析线程从 spawn 到阻塞在栅栏上存在调度延迟；小步重试直至其就绪。
        let resolver_still_blocked = (0..100).any(|_| {
            if release_tx.try_send(()).is_ok() {
                true
            } else {
                std::thread::sleep(Duration::from_millis(50));
                false
            }
        });
        assert!(
            resolver_still_blocked,
            "调用返回时解析线程必须仍在栅栏上阻塞，证明调用未等待解析完成"
        );
        assert!(matches!(
            error,
            AppError::SshConnectionError(message) if message.to_string().contains("DNS 解析超时")
        ));
    }

    /// ConnectPhase 序列化名称保持现有事件契约。
    #[test]
    fn connect_phase_serializes_to_stable_variant_name() {
        assert_eq!(
            serde_json::to_string(&ConnectPhase::Authenticating).unwrap(),
            "\"Authenticating\""
        );
    }

    /// SSH 协议超时保持十秒。
    #[test]
    fn ssh_protocol_timeout_is_ten_seconds() {
        assert_eq!(SSH_PROTOCOL_TIMEOUT_MS, 10_000);
    }

    /// 认证完成后，SFTP 与 Monitoring Exec 的长生命周期会话不得保留建连期的
    /// 十秒 libssh2 阻塞超时；慢速但健康的远端操作可以继续等待。
    #[test]
    fn long_lived_session_clears_protocol_timeout_after_authentication() {
        let session = ssh2::Session::new().expect("应能构造 ssh2 Session");
        session.set_timeout(SSH_PROTOCOL_TIMEOUT_MS);

        clear_long_lived_session_timeout(&session);

        assert_eq!(
            session.timeout(),
            0,
            "SFTP/Exec 长生命周期会话必须关闭 libssh2 阻塞超时"
        );
    }

    /// Exec 输出可能受远端 locale 影响而包含非 UTF-8 字节；这不得使监控采集
    /// 直接失败，输出应以替换字符保留为可解析文本。
    #[test]
    fn exec_output_decodes_non_utf8_bytes_lossily() {
        let output = decode_exec_output(b"CPU_TOTAL=1\xFF\n", 0)
            .expect("含非 UTF-8 字节的成功命令输出仍应返回文本");

        assert_eq!(output, "CPU_TOTAL=1\u{FFFD}\n");
    }

    /// Exec command 的非零退出码不能伪装成成功的部分 stdout；Monitoring 必须收到
    /// 结构化错误并终止本轮采集。
    #[test]
    fn exec_output_rejects_nonzero_exit_status() {
        let error = decode_exec_output(b"partial metric output\n", 17)
            .expect_err("非零退出码必须使 Exec 失败");

        assert!(matches!(
            error,
            AppError::SshProtocolError(detail)
                if detail.to_string().contains("17")
        ));
    }

    // ─── 远端 rename 错误映射 contract ────────────────────────────────────

    /// no-clobber rename 撞上已存在目标：映射为 SftpTargetExists，供发布竞态
    /// 与逐文件确认交互复用。
    #[test]
    fn rename_error_maps_already_exists_to_target_exists_for_no_clobber() {
        let error = ssh2::Error::new(
            ssh2::ErrorCode::SFTP(11), // LIBSSH2_FX_FILE_ALREADY_EXISTS
            "File already exists and SSH_FXP_RENAME_OVERWRITE not specified",
        );

        let mapped = map_sftp_rename_error("/tmp/dst", false, error);

        assert!(
            matches!(&mapped, AppError::SftpTargetExists(path) if path.to_string() == "/tmp/dst"),
            "no-clobber 撞目标应映射为 SftpTargetExists，实际: {mapped:?}"
        );
    }

    /// OpenSSH 的 SFTP v3 no-clobber rename 常把目标已存在报告为
    /// SSH_FX_FAILURE（4）；即使没有英文 "already exists" 文本，也必须进入
    /// 逐文件确认覆盖的 SftpTargetExists 流程。
    #[test]
    fn rename_failure_code_maps_to_target_exists_for_no_clobber() {
        let error = ssh2::Error::new(
            ssh2::ErrorCode::SFTP(4), // LIBSSH2_FX_FAILURE
            "SFTP protocol failure",
        );

        let mapped = map_sftp_rename_error("/tmp/dst", false, error);

        assert!(
            matches!(&mapped, AppError::SftpTargetExists(path) if path.to_string() == "/tmp/dst"),
            "SFTP v3 的 no-clobber failure 必须映射为 SftpTargetExists，实际: {mapped:?}"
        );
    }

    /// 传输层错误即使携带英文 "File already exists" 诊断，也不是 SFTP 目标冲突；
    /// 分类必须只取 SFTP 状态码，避免错误弹出覆盖确认。
    #[test]
    fn rename_does_not_classify_transport_error_by_english_message() {
        let error = ssh2::Error::new(
            ssh2::ErrorCode::Session(-7), // LIBSSH2_ERROR_SOCKET_SEND
            "File already exists",
        );

        let mapped = map_sftp_rename_error("/tmp/dst", false, error);

        assert!(
            matches!(&mapped, AppError::SftpPublishError(detail)
                if detail.to_string().contains("File already exists")),
            "传输错误文本仅保留诊断，不能映射为 SftpTargetExists，实际: {mapped:?}"
        );
    }

    /// SFTP 路径领域错误必须按协议码分类，不能依赖 libssh2/服务端产生的英文文本；
    /// 否则会退化为 SftpChannelError 并无谓触发连接重建。
    #[test]
    fn path_errors_map_sftp_codes_without_english_messages() {
        let missing_file = map_sftp_path_error(
            "/missing-file",
            ssh2::Error::new(ssh2::ErrorCode::SFTP(2), "SFTP protocol failure"),
        );
        let missing_path = map_sftp_path_error(
            "/missing-path",
            ssh2::Error::new(ssh2::ErrorCode::SFTP(10), "SFTP protocol failure"),
        );
        let denied = map_sftp_path_error(
            "/forbidden",
            ssh2::Error::new(ssh2::ErrorCode::SFTP(3), "SFTP protocol failure"),
        );

        assert!(matches!(
            missing_file,
            AppError::SftpPathNotFound(path) if path.to_string() == "/missing-file"
        ));
        assert!(matches!(
            missing_path,
            AppError::SftpPathNotFound(path) if path.to_string() == "/missing-path"
        ));
        assert!(matches!(
            denied,
            AppError::SftpPermissionDenied(path) if path.to_string() == "/forbidden"
        ));
    }

    /// 非 SFTP 的传输失败即使含有英文路径错误文本也不能伪装成领域错误；它应保留
    /// SftpChannelError 以便上层按连接故障处理。
    #[test]
    fn path_error_text_does_not_override_non_sftp_code() {
        let mapped = map_sftp_path_error(
            "/remote/path",
            ssh2::Error::new(
                ssh2::ErrorCode::Session(-7), // LIBSSH2_ERROR_SOCKET_SEND
                "No such file",
            ),
        );

        assert!(matches!(
            mapped,
            AppError::SftpChannelError(detail) if detail.to_string().contains("No such file")
        ));
    }

    /// 覆盖 rename 撞上已存在目标：远端不支持覆盖语义（如 SFTP v3 无覆盖标志），
    /// 映射为 SftpPublishError，detail 说明旧目标保留，绝不先删旧文件。
    #[test]
    fn rename_error_maps_already_exists_to_publish_error_for_overwrite() {
        let error = ssh2::Error::new(
            ssh2::ErrorCode::SFTP(11), // LIBSSH2_FX_FILE_ALREADY_EXISTS
            "File already exists and SSH_FXP_RENAME_OVERWRITE not specified",
        );

        let mapped = map_sftp_rename_error("/tmp/dst", true, error);

        assert!(
            matches!(&mapped, AppError::SftpPublishError(detail)
                if detail.to_string().contains("无法保证安全替换")
                    && detail.to_string().contains("旧目标保留")
                    && detail.to_string().contains("/tmp/dst")),
            "覆盖失败必须保留旧目标并给出结构化发布错误，实际: {mapped:?}"
        );
    }

    /// 其余 rename 失败统一映射为 SftpPublishError，detail 保留底层诊断。
    #[test]
    fn rename_error_maps_other_failures_to_publish_error() {
        let error = ssh2::Error::new(
            ssh2::ErrorCode::SFTP(3), // LIBSSH2_FX_PERMISSION_DENIED
            "Permission denied",
        );

        let mapped = map_sftp_rename_error("/tmp/dst", false, error);

        assert!(
            matches!(&mapped, AppError::SftpPublishError(detail)
                if detail.to_string().contains("远端重命名失败") && detail.to_string().contains("/tmp/dst")),
            "其他 rename 失败应保留结构化发布错误，实际: {mapped:?}"
        );
    }

    /// 从环境变量构造真实 SSH E2E 主机与运行时凭据。
    fn e2e_host() -> (HostConfig, Option<String>, Option<String>) {
        let host = std::env::var("TITAN_SSH_E2E_HOST").expect("缺少 TITAN_SSH_E2E_HOST");
        let username =
            std::env::var("TITAN_SSH_E2E_USERNAME").expect("缺少 TITAN_SSH_E2E_USERNAME");
        let port = std::env::var("TITAN_SSH_E2E_PORT")
            .ok()
            .map(|value| value.parse().expect("TITAN_SSH_E2E_PORT 必须是 u16"))
            .unwrap_or(22);
        let private_key_path = std::env::var("TITAN_SSH_E2E_PRIVATE_KEY_PATH").ok();
        let password = std::env::var("TITAN_SSH_E2E_PASSWORD").ok();
        let passphrase = std::env::var("TITAN_SSH_E2E_PASSPHRASE").ok();
        let auth_type = if private_key_path.is_some() {
            AuthType::PrivateKey
        } else {
            assert!(password.is_some(), "密码认证缺少 TITAN_SSH_E2E_PASSWORD");
            AuthType::Password
        };

        (
            HostConfig {
                id: "ssh-e2e".to_string(),
                name: "ssh-e2e".to_string(),
                host,
                port,
                username,
                auth_type,
                password_ref: password.as_ref().map(|_| "env-password".to_string()),
                private_key_path,
                passphrase_ref: passphrase.as_ref().map(|_| "env-passphrase".to_string()),
                remark: None,
                group: String::new(),
            },
            password,
            passphrase,
        )
    }

    /// 真实 SSH E2E：慢速 SFTP 读取期间 Terminal marker 必须持续到达。
    #[test]
    #[ignore = "需要配置 TITAN_SSH_E2E_* 并访问真实 SSH server"]
    fn real_terminal_stream_continues_during_sftp_transfer() {
        use std::io::Read;
        use std::sync::atomic::{AtomicBool, Ordering};

        let (host, password, passphrase) = e2e_host();
        let mut exec = crate::core::ssh_transport::connect_shared_exec(
            &host,
            password.as_deref(),
            passphrase.as_deref(),
            &allow_all_verifier(),
        )
        .expect("Exec transport 应连接成功")
        .exec_transport();
        let remote_path = format!("/tmp/titan-transport-{}.bin", uuid::Uuid::new_v4());
        exec.execute(&format!(
            "dd if=/dev/zero of={} bs=1048576 count=8 2>/dev/null",
            remote_path
        ))
        .expect("应创建 E2E 远端文件");

        let mut terminal = crate::core::ssh_transport::connect_terminal(
            &host,
            password.as_deref(),
            passphrase.as_deref(),
            &allow_all_verifier(),
            |_| {},
        )
        .expect("Terminal transport 应连接成功");
        let mut sftp = crate::core::ssh_transport::connect_sftp(
            &host,
            password.as_deref(),
            passphrase.as_deref(),
            &allow_all_verifier(),
        )
        .expect("SFTP transport 应连接成功");
        let transfer_started = Arc::new(Barrier::new(2));
        let transfer_done = Arc::new(AtomicBool::new(false));
        let started_for_transfer = transfer_started.clone();
        let done_for_transfer = transfer_done.clone();
        let remote_for_transfer = remote_path.clone();
        let transfer = std::thread::spawn(move || {
            let mut remote = sftp
                .open_read(&remote_for_transfer)
                .expect("应打开 E2E 远端文件");
            started_for_transfer.wait();
            let mut buffer = [0_u8; 32 * 1024];
            while remote.read(&mut buffer).expect("SFTP 读取应成功") > 0 {
                std::thread::sleep(Duration::from_millis(10));
            }
            done_for_transfer.store(true, Ordering::Release);
        });

        transfer_started.wait();
        terminal
            .write("sleep 1; echo TITAN_CONCURRENT_MARKER\n")
            .expect("Terminal 写入应成功");
        let deadline = Instant::now() + Duration::from_secs(6);
        let mut output = String::new();
        let mut marker_arrived_during_transfer = false;
        let mut buffer = [0_u8; 4096];
        while Instant::now() < deadline {
            match terminal.read(&mut buffer) {
                Ok(size) if size > 0 => {
                    output.push_str(&String::from_utf8_lossy(&buffer[..size]));
                    if output.contains("TITAN_CONCURRENT_MARKER")
                        && !transfer_done.load(Ordering::Acquire)
                    {
                        marker_arrived_during_transfer = true;
                        break;
                    }
                }
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                Err(error) => panic!("Terminal 读取失败: {error}"),
            }
            std::thread::sleep(Duration::from_millis(20));
        }

        transfer.join().expect("SFTP 线程应正常退出");
        let _ = terminal.close();
        exec.execute(&format!("rm -f {}", remote_path))
            .expect("应清理 E2E 远端文件");
        assert!(
            marker_arrived_during_transfer,
            "SFTP 完成前应收到 Terminal marker，实际输出: {output}"
        );
    }

    /// 真实 SSH E2E（issue #35）：先采集真实主机 key 并建立可信记录，随后
    /// Terminal streaming、SFTP transfer 与 Monitoring（Exec）全部经真实
    /// HostIdentityService 精确匹配静默放行并正常工作。
    #[test]
    #[ignore = "需要配置 TITAN_SSH_E2E_* 并访问真实 SSH server"]
    fn real_trusted_record_keeps_terminal_sftp_and_monitoring_working() {
        use crate::core::host_identity::{HostIdentityService, HostKeyVerifier, PresentedHostKey};
        use crate::storage::trust_store::{TrustRecord, TrustStore};
        use std::io::{Read, Write};
        use std::sync::{Arc, Mutex};
        use tauri::test::mock_app;

        let (host, password, passphrase) = e2e_host();

        // 1. 首次握手：记录型校验器只采集呈现 key，不做持久化
        let captured: Arc<Mutex<Option<PresentedHostKey>>> = Arc::new(Mutex::new(None));
        let captured_for_verifier = captured.clone();
        let capture_verifier: HostKeyVerifier = Arc::new(move |presented| {
            *captured_for_verifier
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(presented.clone());
            Ok(())
        });
        {
            let mut exec = crate::core::ssh_transport::connect_shared_exec(
                &host,
                password.as_deref(),
                passphrase.as_deref(),
                &capture_verifier,
            )
            .expect("采集连接应成功")
            .exec_transport();
            exec.execute("true").expect("采集连接应可执行命令");
        }
        let presented = captured
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
            .expect("握手应呈现主机 key");

        // 2. 把呈现 key 持久化为可信记录，构建真实 HostIdentityService
        let trust_path = std::env::temp_dir().join(format!(
            "titan-real-trust-{}.known_hosts",
            uuid::Uuid::new_v4()
        ));
        let store = TrustStore::from_file_path(trust_path.clone());
        store
            .upsert(TrustRecord {
                host: presented.host.clone(),
                port: presented.port,
                algorithm: presented.algorithm.clone(),
                blob: presented.blob.clone(),
            })
            .expect("可信记录应写入成功");
        let identity = HostIdentityService::with_trust_store(store);
        let app = mock_app();
        let verifier = identity.verifier(app.handle().clone(), "real-trusted".to_string());

        // 3. Terminal streaming：可信记录精确匹配 → 静默放行，命令往返正常
        let mut terminal = crate::core::ssh_transport::connect_terminal(
            &host,
            password.as_deref(),
            passphrase.as_deref(),
            &verifier,
            |_| {},
        )
        .expect("Terminal 应连接成功");
        terminal
            .write("echo TITAN_TRUSTED_TERMINAL_MARKER\n")
            .expect("Terminal 写入应成功");
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut output = String::new();
        let mut buffer = [0_u8; 4096];
        while Instant::now() < deadline {
            match terminal.read(&mut buffer) {
                Ok(size) if size > 0 => {
                    output.push_str(&String::from_utf8_lossy(&buffer[..size]));
                    if output.contains("TITAN_TRUSTED_TERMINAL_MARKER") {
                        break;
                    }
                }
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                Err(error) => panic!("Terminal 读取失败: {error}"),
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            output.contains("TITAN_TRUSTED_TERMINAL_MARKER"),
            "可信记录放行后 Terminal 应可交互，实际输出: {output}"
        );
        terminal.close().expect("Terminal 应可关闭");

        // 4. SFTP transfer：写入远端文件并读回，内容一致
        let payload = format!("titan-real-sftp-{}", uuid::Uuid::new_v4());
        let remote_path = format!("/tmp/titan-trusted-{}.bin", uuid::Uuid::new_v4());
        let mut sftp = crate::core::ssh_transport::connect_sftp(
            &host,
            password.as_deref(),
            passphrase.as_deref(),
            &verifier,
        )
        .expect("SFTP 应连接成功");
        {
            let mut file = sftp.create(&remote_path).expect("应创建远端文件");
            file.write_all(payload.as_bytes()).expect("应写入远端文件");
            file.flush().expect("应 flush 远端文件");
        }
        {
            let mut file = sftp.open_read(&remote_path).expect("应打开远端文件");
            let mut content = String::new();
            file.read_to_string(&mut content).expect("应读取远端文件");
            assert_eq!(content, payload, "SFTP 往返内容应一致");
        }
        sftp.unlink(&remote_path).expect("应清理远端文件");
        drop(sftp);

        // 5. Monitoring：Exec transport 执行采集命令并返回非空输出
        let mut exec = crate::core::ssh_transport::connect_shared_exec(
            &host,
            password.as_deref(),
            passphrase.as_deref(),
            &verifier,
        )
        .expect("Exec 应连接成功")
        .exec_transport();
        let monitor_output = exec
            .execute("head -n 1 /proc/meminfo 2>/dev/null || vm_stat | head -n 1")
            .expect("监控采集命令应执行成功");
        assert!(!monitor_output.trim().is_empty(), "监控采集应返回非空输出");

        // 6. 全程无 challenge：可信记录精确匹配静默放行
        assert!(
            identity.pending_challenge("real-trusted").is_none(),
            "可信记录精确匹配不得产生 challenge"
        );
        let _ = std::fs::remove_file(&trust_path);
    }
}
