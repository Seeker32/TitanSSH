#[cfg(test)]
mod registry_tests {
    use crate::core::shared_exec_registry::{ExecConnectionEntry, SharedExecRegistry};
    use crate::core::ssh_transport::ExecTransport;
    use crate::core::ssh_transport::test_support::repeating_exec;
    use crate::errors::app_error::{AppError, ErrorDetail};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;

    /// 可观测的内存共享连接条目：派生固定输出 capability，释放时发信号。
    struct FakeEntry {
        output: String,
        dropped: Option<mpsc::Sender<()>>,
    }

    impl FakeEntry {
        /// 构造无释放信号的条目。
        fn new(output: &str) -> Self {
            Self {
                output: output.to_string(),
                dropped: None,
            }
        }

        /// 注册释放信号后返回自身，供回收断言。
        fn with_drop_signal(mut self, dropped: mpsc::Sender<()>) -> Self {
            self.dropped = Some(dropped);
            self
        }
    }

    impl ExecConnectionEntry for FakeEntry {
        /// 派生返回固定输出的 exec capability。
        fn exec_transport(&self) -> ExecTransport {
            repeating_exec(self.output.clone())
        }
    }

    impl Drop for FakeEntry {
        /// 最后一个引用消失时通知测试。
        fn drop(&mut self) {
            if let Some(dropped) = self.dropped.take() {
                let _ = dropped.send(());
            }
        }
    }

    /// 构造稳定的 mock 建连失败错误。
    fn connection_error() -> AppError {
        AppError::SshConnectionError(ErrorDetail::msg("mock 建连失败", Vec::new()))
    }

    /// 首次取用建立并插入连接；同一会话再次取用必须复用同一连接（不重复建连）。
    #[test]
    fn resolve_inserts_on_first_use_and_reuses_entry() {
        let registry = SharedExecRegistry::new();
        let connect_calls = Arc::new(AtomicUsize::new(0));

        let calls_first = connect_calls.clone();
        let mut first = registry
            .resolve("session-1", move || {
                calls_first.fetch_add(1, Ordering::SeqCst);
                Ok(FakeEntry::new("first"))
            })
            .expect("首次取用应建连成功");

        let calls_second = connect_calls.clone();
        let mut second = registry
            .resolve("session-1", move || {
                calls_second.fetch_add(1, Ordering::SeqCst);
                Ok(FakeEntry::new("second"))
            })
            .expect("复用取用应成功");

        assert_eq!(
            connect_calls.load(Ordering::SeqCst),
            1,
            "同一会话的第二次取用不得再次建连"
        );
        assert_eq!(
            first.execute("cmd").expect("取用结果应可执行"),
            "first",
            "首次取用返回先建立的连接"
        );
        assert_eq!(
            second.execute("cmd").expect("取用结果应可执行"),
            "first",
            "复用取用返回注册表中的同一连接"
        );
    }

    /// 显式插入按 sessionId 复用已有连接，并拒绝覆盖先插入者。
    #[test]
    fn insert_keeps_first_entry_for_resolve() {
        let registry = SharedExecRegistry::new();
        assert!(
            registry.insert("session-1", FakeEntry::new("first")),
            "空槽位应允许插入"
        );
        assert!(
            !registry.insert("session-1", FakeEntry::new("second")),
            "已有连接不得被覆盖"
        );

        let mut transport = registry
            .resolve("session-1", || Err::<FakeEntry, _>(connection_error()))
            .expect("resolve 应复用显式插入的连接");
        assert_eq!(transport.execute("cmd").expect("连接应可执行"), "first");
    }

    /// 建连失败不得缓存：下次取用必须重新建连并可成功。
    #[test]
    fn resolve_failure_is_not_cached() {
        let registry = SharedExecRegistry::new();
        let connect_calls = Arc::new(AtomicUsize::new(0));

        let calls = connect_calls.clone();
        let result = registry.resolve("session-1", move || {
            calls.fetch_add(1, Ordering::SeqCst);
            Err::<FakeEntry, _>(connection_error())
        });
        let error = match result {
            Err(error) => error,
            Ok(_) => panic!("建连失败应向上返回错误"),
        };
        assert!(
            matches!(error, AppError::SshConnectionError(_)),
            "错误原样向上传播"
        );

        let calls = connect_calls.clone();
        registry
            .resolve("session-1", move || {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(FakeEntry::new("ok"))
            })
            .expect("失败不得缓存，下次取用应重新建连");

        assert_eq!(connect_calls.load(Ordering::SeqCst), 2);
    }

    /// 不同会话的连接互相隔离：各自建连，回收其一不影响另一个。
    #[test]
    fn sessions_are_isolated() {
        let registry = SharedExecRegistry::new();
        let connect_calls = Arc::new(AtomicUsize::new(0));

        for session_id in ["session-1", "session-2"] {
            let calls = connect_calls.clone();
            registry
                .resolve(session_id, move || {
                    calls.fetch_add(1, Ordering::SeqCst);
                    Ok(FakeEntry::new(session_id))
                })
                .expect("各会话应独立建连");
        }
        assert_eq!(
            connect_calls.load(Ordering::SeqCst),
            2,
            "不同 sessionId 必须各自建立连接"
        );

        assert!(registry.remove("session-1"), "回收存在的会话应返回 true");
        assert!(
            registry.contains("session-2"),
            "回收 session-1 不得影响 session-2"
        );
    }

    /// teardown 回收：remove 移除并释放连接条目，后续取用重新建连。
    #[test]
    fn remove_recycles_entry_and_next_resolve_re_establishes() {
        let registry = SharedExecRegistry::new();
        let (dropped_tx, dropped_rx) = mpsc::channel();
        let connect_calls = Arc::new(AtomicUsize::new(0));

        let calls = connect_calls.clone();
        registry
            .resolve("session-1", move || {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(FakeEntry::new("v1").with_drop_signal(dropped_tx))
            })
            .expect("首次取用应成功");
        assert!(registry.contains("session-1"));

        assert!(registry.remove("session-1"), "回收已注册会话应返回 true");
        assert!(!registry.contains("session-1"), "回收后注册表不得残留条目");
        dropped_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("回收必须释放连接条目（无泄漏）");

        let calls = connect_calls.clone();
        registry
            .resolve("session-1", move || {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(FakeEntry::new("v2"))
            })
            .expect("回收后的取用应重新建连");
        assert_eq!(
            connect_calls.load(Ordering::SeqCst),
            2,
            "回收后取用必须重新建连"
        );
    }

    /// 回收不存在的会话幂等返回 false，不得 panic。
    #[test]
    fn remove_unknown_session_is_idempotent() {
        let registry = SharedExecRegistry::new();
        assert!(!registry.remove("never-existed"), "未知会话回收返回 false");

        registry
            .resolve("session-1", || Ok(FakeEntry::new("ok")))
            .expect("取用应成功");
        assert!(registry.remove("session-1"));
        assert!(!registry.remove("session-1"), "重复回收返回 false");
    }

    /// clear 批量回收全部会话条目（应用退出兜底）。
    #[test]
    fn clear_reclaims_all_entries() {
        let registry = SharedExecRegistry::new();
        let (dropped_tx, dropped_rx) = mpsc::channel();
        let (other_dropped_tx, other_dropped_rx) = mpsc::channel();

        registry
            .resolve("session-1", || {
                Ok(FakeEntry::new("a").with_drop_signal(dropped_tx))
            })
            .expect("取用应成功");
        registry
            .resolve("session-2", || {
                Ok(FakeEntry::new("b").with_drop_signal(other_dropped_tx))
            })
            .expect("取用应成功");

        registry.clear();
        assert!(!registry.contains("session-1") && !registry.contains("session-2"));
        dropped_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("clear 必须释放 session-1 条目");
        other_dropped_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("clear 必须释放 session-2 条目");
    }

    /// 取用返回的 capability 直接可用（透传底层连接行为，无额外包装语义）。
    #[test]
    fn resolved_transport_is_functional() {
        let registry = SharedExecRegistry::new();
        let mut transport = registry
            .resolve("session-1", || Ok(FakeEntry::new("METRIC=1")))
            .expect("取用应成功");
        assert_eq!(
            transport.execute("anything").expect("capability 应可执行"),
            "METRIC=1"
        );
    }

    /// 建连与 teardown 并发时，迟到的建连结果不得重新插回已回收会话。
    #[test]
    fn resolve_does_not_reinsert_after_concurrent_remove() {
        let registry = SharedExecRegistry::new();
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let registry_for_thread = registry.clone();

        let resolver = std::thread::spawn(move || {
            registry_for_thread
                .resolve("session-1", || {
                    started_tx.send(()).expect("测试建连应开始");
                    release_rx.recv().expect("测试建连应被释放");
                    Ok(FakeEntry::new("late"))
                })
                .expect("迟到建连本身仍应可返回 capability")
        });

        started_rx.recv().expect("应观察到建连开始");
        assert!(!registry.remove("session-1"), "尚未插入时回收应返回 false");
        release_tx.send(()).expect("应允许建连完成");
        drop(resolver.join().expect("建连线程不应 panic"));

        assert!(
            !registry.contains("session-1"),
            "teardown 后迟到建连不得重新插回注册表"
        );
    }
}
