#[cfg(test)]
mod tests {
    use crate::core::process_worker::{
        ProcessLoopParams, build_collect_command, parse_process_output_at, run_process_loop_with,
    };
    use crate::core::ssh_transport::test_support::{failing_exec, repeating_exec};
    use crate::errors::app_error::{AppError, ErrorDetail};
    use crate::models::host::{AuthType, HostConfig};
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD;
    use std::collections::HashMap;
    use std::io::Write;
    use std::process::{Command, Stdio};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    /// 生成进程 worker 测试使用的最小主机配置。
    fn host() -> HostConfig {
        HostConfig {
            id: "host-1".to_string(),
            name: "test".to_string(),
            host: "127.0.0.1".to_string(),
            port: 22,
            username: "ops".to_string(),
            auth_type: AuthType::Password,
            password_ref: None,
            private_key_path: None,
            passphrase_ref: None,
            remark: None,
            group: String::new(),
        }
    }

    /// 将测试字段编码成远端脚本使用的 base64 表示。
    fn field(value: &str) -> String {
        STANDARD.encode(value)
    }

    /// 生成一条稳定的伪造进程记录。
    fn record(pid: u32, ppid: u32, state: &str, utime: u64, stime: u64, rss: u64) -> String {
        format!(
            "P\t{pid}\t{ppid}\t{state}\t{utime}\t{stime}\t{rss}\t{}\t{}\t{}",
            field("ops"),
            field("worker"),
            field("worker --serve")
        )
    }

    /// 首次采样没有 CPU 基准，但仍返回 RSS 与已解码字段。
    #[test]
    fn parses_process_records_and_leaves_first_cpu_unknown() {
        let output = format!(
            "PLATFORM=linux\nHZ=100\n{}\n",
            record(42, 1, "R", 100, 50, 8192)
        );
        let (snapshot, samples) =
            parse_process_output_at("session-1", &output, &HashMap::new(), None, 1_000)
                .expect("valid /proc output should parse");

        assert_eq!(snapshot.total_count, 1);
        assert_eq!(snapshot.processes[0].pid, 42);
        assert_eq!(snapshot.processes[0].ppid, 1);
        assert_eq!(snapshot.processes[0].user, "ops");
        assert_eq!(snapshot.processes[0].command_line, "worker --serve");
        assert_eq!(snapshot.processes[0].memory_bytes, Some(8192));
        assert_eq!(snapshot.processes[0].cpu_percent, None);
        assert_eq!(samples.get(&42), Some(&150));
    }

    /// CPU 差值使用远端 tick 频率与真实毫秒间隔换算。
    #[test]
    fn computes_cpu_percent_from_tick_delta_and_drops_disappeared_processes() {
        let first = format!(
            "PLATFORM=linux\nHZ=100\n{}\n{}\n",
            record(42, 1, "R", 100, 50, 8192),
            record(9, 1, "S", 2, 3, 100)
        );
        let (_, samples) =
            parse_process_output_at("session-1", &first, &HashMap::new(), None, 1_000).unwrap();

        let second = format!(
            "PLATFORM=linux\nHZ=100\n{}\n",
            record(42, 1, "S", 130, 70, 9000)
        );
        let (snapshot, current) =
            parse_process_output_at("session-1", &second, &samples, Some(1_000), 3_000).unwrap();

        assert_eq!(snapshot.processes.len(), 1);
        assert_eq!(snapshot.processes[0].cpu_percent, Some(25.0));
        assert_eq!(snapshot.processes[0].memory_bytes, Some(9000));
        assert_eq!(current.len(), 1);
        assert!(current.contains_key(&42));
        assert!(!current.contains_key(&9));
    }

    /// worker 接受注入的 exec provider，并在一轮快照后正常停机。
    #[test]
    fn worker_uses_injected_provider_and_emits_snapshot() {
        let shutdown = Arc::new(AtomicBool::new(false));
        let snapshots = Arc::new(Mutex::new(Vec::new()));
        let output = format!(
            "PLATFORM=linux\nHZ=100\n{}\n",
            record(42, 1, "R", 100, 50, 8192)
        );
        let snapshots_for_callback = snapshots.clone();
        let shutdown_for_callback = shutdown.clone();

        run_process_loop_with(
            move |_, _, _| Ok(repeating_exec(output.clone())),
            ProcessLoopParams {
                host: host(),
                password: None,
                passphrase: None,
                session_id: "session-1".to_string(),
                shutdown,
            },
            move |snapshot| {
                snapshots_for_callback.lock().unwrap().push(snapshot);
                shutdown_for_callback.store(true, Ordering::Release);
            },
            |error| panic!("unexpected process worker error: {error}"),
        );

        let snapshots = snapshots.lock().unwrap();
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].processes[0].pid, 42);
    }

    /// 连接 provider 失败会经错误缝返回，且不会被缓存。
    #[test]
    fn worker_reports_provider_failure() {
        let errors = Arc::new(Mutex::new(Vec::new()));
        let errors_for_callback = errors.clone();

        run_process_loop_with(
            |_, _, _| {
                Err(AppError::SshConnectionError(ErrorDetail::msg(
                    "mock connection failed",
                    Vec::new(),
                )))
            },
            ProcessLoopParams {
                host: host(),
                password: None,
                passphrase: None,
                session_id: "session-1".to_string(),
                shutdown: Arc::new(AtomicBool::new(false)),
            },
            |_| panic!("unexpected process snapshot"),
            move |error| errors_for_callback.lock().unwrap().push(error.code()),
        );

        assert_eq!(errors.lock().unwrap().as_slice(), &["SshConnectionError"]);
    }

    /// 远端采集命令使用 base64 包装，并包含 Linux /proc 遍历。
    #[test]
    fn collector_command_is_shell_safe_and_reads_proc() {
        let command = build_collect_command();
        assert!(command.starts_with("echo "));
        assert!(!command.contains("/proc"));
        let encoded = command
            .strip_prefix("echo ")
            .and_then(|value| value.strip_suffix(" | base64 -d | sh"))
            .expect("collector command should use the standard wrapper");
        let script = String::from_utf8(STANDARD.decode(encoded).unwrap()).unwrap();
        assert!(script.contains("/proc/[0-9]*"));
        assert!(script.contains("$proc/stat"));
        assert!(script.contains("PAGE_SIZE=$(getconf PAGESIZE"));
        assert!(script.contains("rss_bytes=$((rss_pages * PAGE_SIZE))"));
        assert!(script.contains("utime=${12}"));
        assert!(script.contains("stime=${13}"));
        assert!(script.contains("rss_pages=${22}"));

        let mut shell = Command::new("sh")
            .arg("-n")
            .stdin(Stdio::piped())
            .spawn()
            .expect("sh should be available for syntax validation");
        shell
            .stdin
            .take()
            .expect("syntax checker stdin should be available")
            .write_all(script.as_bytes())
            .expect("script should be accepted by stdin");
        assert!(shell.wait().expect("syntax checker should exit").success());
    }

    /// 非 Linux SSH 目标返回明确的结构化不支持错误。
    #[test]
    fn parser_rejects_non_linux_remote_target() {
        let error = parse_process_output_at(
            "session-1",
            "PLATFORM=freebsd\n",
            &HashMap::new(),
            None,
            1_000,
        )
        .expect_err("non-Linux remote target should be rejected");
        assert_eq!(error.code(), "ProcessMonitoringUnsupported");
    }

    /// 缺失采样基准时返回结构化解析错误，而不是伪造空快照。
    #[test]
    fn parser_rejects_missing_tick_frequency() {
        let error = parse_process_output_at(
            "session-1",
            "PLATFORM=linux\n",
            &HashMap::new(),
            None,
            1_000,
        )
        .expect_err("missing HZ should be rejected");
        assert_eq!(error.code(), "ProcessCollectionError");
    }

    /// 缺失远端平台标记时返回结构化解析错误。
    #[test]
    fn parser_rejects_missing_platform_marker() {
        let error = parse_process_output_at("session-1", "HZ=100\n", &HashMap::new(), None, 1_000)
            .expect_err("missing platform marker should be rejected");
        assert_eq!(error.code(), "ProcessCollectionError");
    }

    /// 非数字 HZ 不得被当作有效采样频率。
    #[test]
    fn parser_rejects_invalid_tick_frequency() {
        let error = parse_process_output_at(
            "session-1",
            "PLATFORM=linux\nHZ=not-a-number\n",
            &HashMap::new(),
            None,
            1_000,
        )
        .expect_err("invalid HZ should be rejected");
        assert_eq!(error.code(), "ProcessCollectionError");
    }

    /// 不完整或非法的进程记录只被丢弃，不污染有效快照。
    #[test]
    fn parser_skips_malformed_process_records() {
        let output = format!(
            "PLATFORM=linux\nHZ=100\nnot-a-process-record\n{}\n",
            record(42, 1, "R", 100, 50, 8192)
        );
        let (snapshot, _) =
            parse_process_output_at("session-1", &output, &HashMap::new(), None, 1_000)
                .expect("malformed records should not fail the full sample");
        assert_eq!(snapshot.total_count, 1);
        assert_eq!(snapshot.processes[0].pid, 42);
    }

    /// 远端 exec 失败会停止 worker 并返回底层结构化错误。
    #[test]
    fn worker_reports_collection_failure() {
        let errors = Arc::new(Mutex::new(Vec::new()));
        let errors_for_callback = errors.clone();
        run_process_loop_with(
            |_, _, _| Ok(failing_exec()),
            ProcessLoopParams {
                host: host(),
                password: None,
                passphrase: None,
                session_id: "session-1".to_string(),
                shutdown: Arc::new(AtomicBool::new(false)),
            },
            |_| panic!("failed exec must not emit a snapshot"),
            move |error| errors_for_callback.lock().unwrap().push(error.code()),
        );
        assert_eq!(errors.lock().unwrap().as_slice(), &["SshConnectionError"]);
    }

    /// 停机标志预先置位时不得建立连接或发出任何回调。
    #[test]
    fn worker_honors_shutdown_before_connecting() {
        let shutdown = Arc::new(AtomicBool::new(true));
        let resolved = Arc::new(AtomicBool::new(false));
        let resolved_for_provider = resolved.clone();
        run_process_loop_with(
            move |_, _, _| {
                resolved_for_provider.store(true, Ordering::Release);
                Ok(repeating_exec(String::new()))
            },
            ProcessLoopParams {
                host: host(),
                password: None,
                passphrase: None,
                session_id: "session-1".to_string(),
                shutdown,
            },
            |_| panic!("shutdown worker must not emit a snapshot"),
            |_| panic!("shutdown worker must not emit an error"),
        );
        assert!(!resolved.load(Ordering::Acquire));
    }
}
