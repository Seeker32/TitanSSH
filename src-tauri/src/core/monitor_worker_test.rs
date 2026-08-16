#[cfg(test)]
mod loop_tests {
    use crate::core::monitor_worker::*;
    use crate::core::ssh_transport::ExecTransport;
    use crate::core::ssh_transport::test_support::one_shot_exec;
    use crate::errors::app_error::{AppError, ErrorDetail};
    use crate::models::host::{AuthType, HostConfig};
    use crate::models::monitor::MonitorSnapshot;
    use std::sync::atomic::AtomicBool;
    use std::sync::{Arc, Mutex};

    /// 构造测试用 HostConfig
    fn make_host() -> HostConfig {
        HostConfig {
            id: "h1".to_string(),
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

    /// 构造测试用监控循环参数
    fn make_params(shutdown: Arc<AtomicBool>) -> MonitorLoopParams {
        MonitorLoopParams {
            host: make_host(),
            password: Some("pw".to_string()),
            passphrase: None,
            session_id: "session-1".to_string(),
            shutdown,
        }
    }

    /// 监控连接与其他 capability 一样先经过主机身份统一校验：
    /// 校验被拒绝时监控连接失败（on_error 携带 HostKeyRejected），不进入采集。
    #[test]
    fn rejected_host_identity_fails_monitor_before_collection() {
        use crate::core::host_identity::PresentedHostKey;
        use std::sync::atomic::Ordering;

        let snapshots: Arc<Mutex<Vec<MonitorSnapshot>>> = Arc::new(Mutex::new(vec![]));
        let errors: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(vec![]));
        let shutdown = Arc::new(AtomicBool::new(false));

        let snap_ref = Arc::clone(&snapshots);
        let err_ref = Arc::clone(&errors);

        // 模拟生产 transport 顺序：握手后、认证前调用统一校验器
        let verifier: HostKeyVerifier = Arc::new(|_presented: &PresentedHostKey| {
            Err(AppError::HostKeyRejected("10.0.0.8:22".to_string().into()))
        });
        let connect_fn = move |_host: &HostConfig,
                               _pw: Option<&str>,
                               _pp: Option<&str>|
              -> Result<ExecTransport, AppError> {
            verifier(&PresentedHostKey {
                host: "10.0.0.8".to_string(),
                port: 22,
                algorithm: "ssh-ed25519".to_string(),
                fingerprint: "SHA256:monitor".to_string(),
                blob: b"blob".to_vec(),
            })?;
            unreachable!("主机身份被拒绝时不得建立监控连接");
        };

        run_monitor_loop_with(
            connect_fn,
            make_params(shutdown),
            move |snapshot| snap_ref.lock().unwrap().push(snapshot),
            move |error| err_ref.lock().unwrap().push(error.to_string()),
        );

        assert_eq!(snapshots.lock().unwrap().len(), 0);
        let errors = errors.lock().unwrap();
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("主机身份"), "错误应携带主机身份语义");
        let _ = Ordering::Relaxed;
    }

    /// 连接失败时 on_error 被调用，on_snapshot 不被调用
    #[test]
    fn connect_failure_calls_on_error_not_on_snapshot() {
        let snapshots: Arc<Mutex<Vec<MonitorSnapshot>>> = Arc::new(Mutex::new(vec![]));
        let errors: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(vec![]));
        let shutdown = Arc::new(AtomicBool::new(false));

        let snap_ref = Arc::clone(&snapshots);
        let err_ref = Arc::clone(&errors);

        let connect_fn = |_host: &HostConfig, _pw: Option<&str>, _pp: Option<&str>| {
            Err::<ExecTransport, AppError>(AppError::SshConnectionError(ErrorDetail::msg(
                "mock 连接失败",
                Vec::new(),
            )))
        };

        run_monitor_loop_with(
            connect_fn,
            make_params(shutdown),
            move |snap| {
                snap_ref.lock().unwrap().push(snap);
            },
            move |err| {
                err_ref.lock().unwrap().push(err.to_string());
            },
        );

        assert_eq!(
            snapshots.lock().unwrap().len(),
            0,
            "连接失败时不应调用 on_snapshot"
        );
        assert_eq!(
            errors.lock().unwrap().len(),
            1,
            "连接失败时应调用一次 on_error"
        );
        assert!(errors.lock().unwrap()[0].contains("mock 连接失败"));
    }

    /// shutdown=true 时循环不执行，on_error 不被调用
    #[test]
    fn shutdown_before_connect_exits_cleanly() {
        let errors: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(vec![]));
        let shutdown = Arc::new(AtomicBool::new(true));

        let err_ref = Arc::clone(&errors);
        let connect_fn = |_: &HostConfig, _: Option<&str>, _: Option<&str>| {
            Err::<ExecTransport, AppError>(AppError::SshConnectionError(ErrorDetail::msg(
                "不应被调用",
                Vec::new(),
            )))
        };

        run_monitor_loop_with(
            connect_fn,
            make_params(shutdown),
            |_| {},
            move |err| {
                err_ref.lock().unwrap().push(err.to_string());
            },
        );

        assert_eq!(
            errors.lock().unwrap().len(),
            0,
            "shutdown=true 时不应调用 on_error"
        );
    }

    /// Monitoring 只通过 Exec capability 采集并发布结构化快照。
    #[test]
    fn exec_capability_produces_monitor_snapshot() {
        let shutdown = Arc::new(AtomicBool::new(false));
        let snapshots = Arc::new(Mutex::new(Vec::new()));
        let snapshots_for_callback = snapshots.clone();
        let shutdown_for_transport = shutdown.clone();

        run_monitor_loop_with(
            move |_, _, _| {
                Ok(one_shot_exec(
                    "CPU_TOTAL=100\nCPU_IDLE=20\nMEM_TOTAL_KB=1000\nMEM_AVAILABLE_KB=500\nDISK=25\nDISK_AVAIL=750\nDISK_TOTAL=1000".to_string(),
                    shutdown_for_transport,
                ))
            },
            make_params(shutdown),
            move |snapshot| snapshots_for_callback.lock().unwrap().push(snapshot),
            |_| panic!("采集成功时不应调用 on_error"),
        );

        let snapshots = snapshots.lock().unwrap();
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].session_id, "session-1");
        assert_eq!(snapshots[0].disk_usage, Some(25.0));
    }
}
