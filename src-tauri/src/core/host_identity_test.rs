#[cfg(test)]
mod tests {
    use crate::core::host_identity::*;
    use crate::models::host_identity::TrustedHostInfo;
    use crate::models::session::HostIdentityChallengeDismissed;
    use crate::storage::trust_store::{TrustRecord, TrustStore};
    use serde_json::Value;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::thread;
    use std::time::{Duration, Instant};
    use tauri::Listener;
    use tauri::test::mock_app;
    use uuid::Uuid;

    fn make_presented(fingerprint: &str) -> PresentedHostKey {
        PresentedHostKey {
            host: "10.0.0.8".to_string(),
            port: 22,
            algorithm: "ssh-ed25519".to_string(),
            fingerprint: fingerprint.to_string(),
            blob: b"blob".to_vec(),
        }
    }

    /// 隔离的临时信任存储路径（默认不存在，等价空信任存储）。
    fn temp_trust_path() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("titan-identity-trust-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).expect("temp dir should be created");
        dir.join("known_hosts")
    }

    /// 构造带信任存储的服务并预置指定记录。
    fn service_with_record(record: TrustRecord) -> (HostIdentityService, PathBuf) {
        let path = temp_trust_path();
        let store = TrustStore::from_file_path(path.clone());
        store.upsert(record).expect("预置记录应写入成功");
        (HostIdentityService::with_trust_store(store), path)
    }

    /// 等待指定 Session 出现 pending challenge（超时则 panic）。
    fn wait_pending(service: &HostIdentityService, session_id: &str) -> HostIdentityChallenge {
        let deadline = Instant::now() + Duration::from_secs(2);
        while service.pending_challenge(session_id).is_none() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        service
            .pending_challenge(session_id)
            .expect("challenge 已创建")
    }

    /// verify 的持久化命中分支不得在持有 trust_store 锁时进入 lookup/重取 state 锁：
    /// 与 accept_and_save 的 state → trust store 锁序构成 AB-BA 死锁。
    ///
    /// 用 FIFO 文件让 store.lookup 的首次文件读取确定性阻塞，并用非阻塞 writer
    /// 探测 verify 是否已进入 lookup（reader 就位）：store 锁占用 + reader 就位
    /// 同时成立即锁序违规；store 锁空闲 + reader 就位即修复后的正确行为。
    #[test]
    fn verify_persisted_hit_releases_store_lock_before_state_lock() {
        use std::io::Write as _;
        use std::os::unix::fs::OpenOptionsExt;
        use std::process::Command;

        let dir = std::env::temp_dir().join(format!("titan-identity-fifo-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).expect("temp dir should be created");
        let fifo = dir.join("known_hosts");
        assert!(
            Command::new("mkfifo")
                .arg(&fifo)
                .status()
                .expect("应能创建 FIFO")
                .success()
        );

        let app = mock_app();
        // 缓存未加载（records=None）：verify 的 lookup 触发文件读取并阻塞在 FIFO 上
        let service = HostIdentityService::with_trust_store_path(fifo.clone());
        let presented = make_presented("SHA256:match");
        let verifier = service.verifier(app.handle().clone(), "session-1".to_string());
        let handle = thread::spawn({
            let verifier = verifier.clone();
            let presented = presented.clone();
            move || verifier(&presented)
        });

        // 非阻塞 writer 探测 reader 就位：O_NONBLOCK（Linux 全架构 0x800）下
        // FIFO 已有 reader 时 writer 打开立即成功，无 reader 时以 ENXIO 失败。
        // 不依赖任何时序假设，只观察「verify 阻塞在 lookup」这一确定性状态。
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut probe_writer = None;
        while Instant::now() < deadline {
            match fs::OpenOptions::new()
                .write(true)
                .custom_flags(0x800)
                .open(&fifo)
            {
                Ok(writer) => {
                    probe_writer = Some(writer);
                    // verify 已阻塞在 lookup（reader 就位）；此时 store 锁必须空闲
                    assert!(
                        service.trust_store.try_lock().is_ok(),
                        "verify 在持有 trust_store 锁时进入 lookup，\
                         与 accept_and_save 的 state → store 锁序构成 AB-BA 死锁"
                    );
                    break;
                }
                Err(_) => thread::sleep(Duration::from_millis(10)),
            }
        }
        let mut writer = probe_writer.expect("verify 应在期限内进入 lookup");

        // 解除 lookup 阻塞：写入与呈现匹配的记录，verify 应继续并放行
        writer
            .write_all(b"10.0.0.8 ssh-ed25519 YmxvYg\n")
            .expect("应能写入记录");
        drop(writer);
        handle
            .join()
            .expect("verify 线程不应 panic")
            .expect("匹配记录应放行");
        let _ = fs::remove_dir_all(&dir);
    }

    /// 接受并保存的 known_hosts 写入可能在慢盘上阻塞；此时 global state 锁必须
    /// 已释放，避免其他 Session 的 verify、关闭或决定操作被整个写盘周期卡住。
    #[test]
    fn accept_and_save_releases_state_lock_while_trust_store_write_blocks() {
        use std::os::unix::fs::OpenOptionsExt;
        use std::process::Command;

        let dir = std::env::temp_dir().join(format!("titan-identity-save-fifo-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).expect("temp dir should be created");
        let fifo = dir.join("known_hosts");
        assert!(
            Command::new("mkfifo")
                .arg(&fifo)
                .status()
                .expect("应能创建 FIFO")
                .success()
        );

        let app = mock_app();
        // 先在无持久化存储的状态创建 challenge，避免 verify 本身读取 FIFO。
        let service = HostIdentityService::new();
        let verifier = service.verifier(app.handle().clone(), "session-1".to_string());
        let waiter = thread::spawn(move || verifier(&make_presented("SHA256:save-block")));
        let challenge = wait_pending(&service, "session-1");
        *service.trust_store.lock().expect("信任存储锁应可获取") =
            Some(TrustStore::from_file_path(fifo.clone()));

        let save = {
            let service = service.clone();
            let app_handle = app.handle().clone();
            let challenge_id = challenge.challenge_id.clone();
            thread::spawn(move || service.accept_and_save(&app_handle, &challenge_id))
        };

        // 非阻塞 writer 成功意味着 upsert 已在 FIFO 上等待读取；此刻保存操作的
        // 文件 I/O 正在进行，state 锁若仍被持有则说明会阻塞所有其他 Session。
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut probe_writer = None;
        while Instant::now() < deadline {
            match fs::OpenOptions::new()
                .write(true)
                .custom_flags(0x800)
                .open(&fifo)
            {
                Ok(writer) => {
                    probe_writer = Some(writer);
                    assert!(
                        service.state.try_lock().is_ok(),
                        "accept_and_save 写入 known_hosts 时不得持有 global state 锁"
                    );
                    break;
                }
                Err(_) => thread::sleep(Duration::from_millis(10)),
            }
        }
        drop(probe_writer.expect("保存操作应在期限内进入信任存储读取"));

        save.join()
            .expect("保存线程不应 panic")
            .expect("解除 FIFO 阻塞后保存应成功");
        waiter
            .join()
            .expect("等待线程不应 panic")
            .expect("保存成功后应放行认证");
        let _ = fs::remove_dir_all(&dir);
    }

    /// 已保存 key 精确匹配：verify 在认证前直接放行，不产生 challenge。
    #[test]
    fn saved_key_exact_match_skips_challenge() {
        let app = mock_app();
        let events = Arc::new(AtomicUsize::new(0));
        let counter = events.clone();
        app.listen("host-identity:challenge", move |_| {
            counter.fetch_add(1, Ordering::Relaxed);
        });
        let (service, _path) = service_with_record(TrustRecord {
            host: "10.0.0.8".to_string(),
            port: 22,
            algorithm: "ssh-ed25519".to_string(),
            blob: b"blob".to_vec(),
        });
        let verifier = service.verifier(app.handle().clone(), "session-1".to_string());

        verifier(&make_presented("SHA256:match")).expect("匹配记录应静默放行");
        assert!(service.pending_challenge("session-1").is_none());
        assert_eq!(events.load(Ordering::Relaxed), 0, "匹配时不产生 challenge");
    }

    /// 过期的取消标记不再拒绝校验：保留期后标记被清理（内存有界），
    /// 复用该 session_id 的校验器按未知主机正常走 challenge 流程。
    #[test]
    fn expired_cancelled_marker_stops_rejecting() {
        let app = mock_app();
        let service = HostIdentityService::new();
        // 直接注入过期的取消标记（模拟很久以前关闭的 Session）
        {
            let mut state = service.state.lock().unwrap();
            state.cancelled.insert(
                "session-old".to_string(),
                Instant::now() - CANCELLED_RETENTION - Duration::from_secs(1),
            );
        }
        let verifier = service.verifier(app.handle().clone(), "session-old".to_string());
        let presented = make_presented("SHA256:old");
        let waiter = thread::spawn(move || verifier(&presented));
        let challenge = wait_pending(&service, "session-old");
        service.reject(&challenge.challenge_id).unwrap();
        assert_eq!(
            waiter.join().unwrap().unwrap_err().code(),
            "HostKeyRejected",
            "过期标记不得以取消错误拒绝校验"
        );
        // 过期条目在检查时被清理，集合保持有界
        assert!(service.state.lock().unwrap().cancelled.is_empty());
    }

    /// cancel_session 插入新标记时清理过期的旧标记。
    #[test]
    fn cancel_session_prunes_expired_markers() {
        let app = mock_app();
        let service = HostIdentityService::new();
        {
            let mut state = service.state.lock().unwrap();
            state.cancelled.insert(
                "session-old".to_string(),
                Instant::now() - CANCELLED_RETENTION - Duration::from_secs(1),
            );
        }
        service.cancel_session(app.handle(), "session-new");
        let state = service.state.lock().unwrap();
        assert!(
            !state.cancelled.contains_key("session-old"),
            "过期的旧标记应在插入时被清理"
        );
        assert!(state.cancelled.contains_key("session-new"), "新标记应保留");
    }

    /// 应用退出路径：cancel_all 清空取消标记，集合不再持有已退出会话的历史。
    #[test]
    fn cancel_all_clears_cancelled_markers() {
        let app = mock_app();
        let service = HostIdentityService::new();
        service.cancel_session(app.handle(), "session-1");
        service.cancel_session(app.handle(), "session-2");
        service.cancel_all(app.handle());
        assert!(service.state.lock().unwrap().cancelled.is_empty());
    }

    /// 已关闭 Session 的迟到校验器不得借持久化信任继续认证：取消检查先于匹配放行。
    #[test]
    fn cancelled_session_fails_even_when_key_is_saved() {
        let app = mock_app();
        let (service, _path) = service_with_record(TrustRecord {
            host: "10.0.0.8".to_string(),
            port: 22,
            algorithm: "ssh-ed25519".to_string(),
            blob: b"blob".to_vec(),
        });
        let verifier = service.verifier(app.handle().clone(), "session-gone".to_string());

        service.cancel_session(app.handle(), "session-gone");
        let error = verifier(&make_presented("SHA256:match")).unwrap_err();
        assert_eq!(
            error.code(),
            "HostKeyVerificationCancelled",
            "关闭后的 Session 即使 key 已保存也不得进入认证"
        );
        assert!(service.pending_challenge("session-gone").is_none());
    }

    /// 已保存 key 的匹配是精确的：host 拼写、端口、算法或公钥任一不同都产生 challenge。
    #[test]
    fn saved_key_match_is_exact_on_all_fields() {
        let app = mock_app();
        let (service, _path) = service_with_record(TrustRecord {
            host: "10.0.0.8".to_string(),
            port: 22,
            algorithm: "ssh-ed25519".to_string(),
            blob: b"blob".to_vec(),
        });
        let verifier = service.verifier(app.handle().clone(), "session-1".to_string());

        let variants = [
            PresentedHostKey {
                host: "10.0.0.9".to_string(),
                ..make_presented("SHA256:a")
            },
            PresentedHostKey {
                port: 2222,
                ..make_presented("SHA256:b")
            },
            PresentedHostKey {
                algorithm: "ssh-rsa".to_string(),
                ..make_presented("SHA256:c")
            },
            PresentedHostKey {
                blob: b"other".to_vec(),
                ..make_presented("SHA256:d")
            },
        ];
        for presented in variants {
            let verifier = verifier.clone();
            let waiter = thread::spawn(move || verifier(&presented));
            let challenge = wait_pending(&service, "session-1");
            service.reject(&challenge.challenge_id).unwrap();
            assert_eq!(
                waiter.join().unwrap().unwrap_err().code(),
                "HostKeyRejected",
                "任一字段不匹配都必须重新确认"
            );
        }
    }

    /// 信任文件缺失：等价空信任存储，未知主机仍走 challenge。
    #[test]
    fn missing_trust_file_means_empty_store() {
        let app = mock_app();
        let service = HostIdentityService::with_trust_store_path(temp_trust_path());
        let verifier = service.verifier(app.handle().clone(), "session-1".to_string());
        let presented = make_presented("SHA256:empty");
        let waiter = thread::spawn(move || verifier(&presented));
        let challenge = wait_pending(&service, "session-1");
        service.accept(&challenge.challenge_id).unwrap();
        waiter.join().unwrap().expect("空信任存储下接受后放行");
    }

    /// 信任文件损坏：fail-closed，verify 以 TrustStoreError 失败，不产生 challenge。
    #[test]
    fn corrupt_trust_store_fails_closed_without_challenge() {
        let app = mock_app();
        let path = temp_trust_path();
        fs::write(&path, "10.0.0.8 ssh-ed25519\n").unwrap();
        let service = HostIdentityService::with_trust_store_path(path);
        let verifier = service.verifier(app.handle().clone(), "session-1".to_string());

        let error = verifier(&make_presented("SHA256:corrupt")).unwrap_err();
        assert_eq!(error.code(), "TrustStoreError");
        assert!(
            service.pending_challenge("session-1").is_none(),
            "fail-closed 不得产生 challenge"
        );
    }

    /// list_trusted_hosts 返回 typed DTO：endpoint 字段精确、指纹由后端从公钥 blob 计算。
    #[test]
    fn list_trusted_hosts_returns_typed_dto_with_fingerprint() {
        let (service, _path) = service_with_record(TrustRecord {
            host: "10.0.0.8".to_string(),
            port: 2222,
            algorithm: "ssh-ed25519".to_string(),
            blob: b"blob".to_vec(),
        });

        let hosts = service.list_trusted_hosts().unwrap();
        assert_eq!(
            hosts,
            vec![TrustedHostInfo {
                host: "10.0.0.8".to_string(),
                port: 2222,
                algorithm: "ssh-ed25519".to_string(),
                fingerprint: fingerprint_sha256(b"blob"),
            }]
        );
        assert_eq!(
            hosts[0].fingerprint,
            "SHA256:+iyMxPKBdrvu1Lc231aaNMec03I+nsQvlnS01GrGuLg"
        );
    }

    /// 清单按 host 字典序 + port 稳定排序；读取/解析失败以 TrustStoreError 显式返回，
    /// 绝不伪装成空列表；未初始化（仅测试路径）等价空信任存储。
    #[test]
    fn list_trusted_hosts_stable_order_and_store_error_propagation() {
        let path = temp_trust_path();
        let store = TrustStore::from_file_path(path.clone());
        store
            .upsert(TrustRecord {
                host: "b.example.com".to_string(),
                port: 22,
                algorithm: "ssh-rsa".to_string(),
                blob: b"blob-b".to_vec(),
            })
            .unwrap();
        store
            .upsert(TrustRecord {
                host: "a.example.com".to_string(),
                port: 2222,
                algorithm: "ssh-ed25519".to_string(),
                blob: b"blob-a".to_vec(),
            })
            .unwrap();
        let service = HostIdentityService::with_trust_store(store);

        let hosts = service.list_trusted_hosts().unwrap();
        assert_eq!(
            hosts
                .iter()
                .map(|info| (info.host.as_str(), info.port))
                .collect::<Vec<_>>(),
            vec![("a.example.com", 2222), ("b.example.com", 22)]
        );

        // 文件损坏：结构化 TrustStoreError，不得伪装成空列表
        fs::write(&path, "10.0.0.8 ssh-ed25519\n").unwrap();
        let corrupt = HostIdentityService::with_trust_store_path(path);
        assert_eq!(
            corrupt.list_trusted_hosts().unwrap_err().code(),
            "TrustStoreError"
        );

        // 未初始化信任存储（仅测试路径）：等价空清单
        assert_eq!(
            HostIdentityService::new().list_trusted_hosts().unwrap(),
            Vec::new()
        );
    }

    /// 接受并保存：等待连接继续认证，记录持久化，后续 Session 静默复用。
    #[test]
    fn accept_and_save_persists_and_releases_waiters() {
        let app = mock_app();
        let (service, path) = service_with_record(TrustRecord {
            host: "10.0.0.9".to_string(),
            port: 22,
            algorithm: "ssh-ed25519".to_string(),
            blob: b"unrelated".to_vec(),
        });
        let verifier = service.verifier(app.handle().clone(), "session-1".to_string());
        let presented = make_presented("SHA256:save");
        let waiter = thread::spawn(move || verifier(&presented));
        let challenge = wait_pending(&service, "session-1");

        service
            .accept_and_save(app.handle(), &challenge.challenge_id)
            .unwrap();
        waiter.join().unwrap().expect("保存成功后放行认证");
        // 磁盘真实内容：endpoint 记录已写入（含原无关记录，不丢其他 endpoint）
        let records = TrustStore::from_file_path(path).reload().unwrap();
        assert_eq!(records.len(), 2);
        let saved = records
            .iter()
            .find(|record| record.host == "10.0.0.8" && record.port == 22)
            .expect("endpoint 记录已保存");
        assert_eq!(saved.algorithm, "ssh-ed25519");
        assert_eq!(saved.blob, b"blob");

        // 后续 Runtime Session：匹配记录静默放行，不再提示
        let verifier_b = service.verifier(app.handle().clone(), "session-b".to_string());
        verifier_b(&make_presented("SHA256:save")).expect("保存后新 Session 不再提示");
    }

    /// 同一 endpoint 已保存不同 key：仍产生 challenge；接受并保存覆盖为当前记录。
    #[test]
    fn accept_and_save_overwrites_previous_record_for_endpoint() {
        let app = mock_app();
        let (service, path) = service_with_record(TrustRecord {
            host: "10.0.0.8".to_string(),
            port: 22,
            algorithm: "ssh-rsa".to_string(),
            blob: b"old-key".to_vec(),
        });
        let verifier = service.verifier(app.handle().clone(), "session-1".to_string());
        let presented = make_presented("SHA256:rotate");
        let waiter = thread::spawn(move || verifier(&presented));
        let challenge = wait_pending(&service, "session-1");

        service
            .accept_and_save(app.handle(), &challenge.challenge_id)
            .unwrap();
        waiter.join().unwrap().unwrap();
        let records = TrustStore::from_file_path(path).reload().unwrap();
        assert_eq!(records.len(), 1, "同一 endpoint 只保留一条记录");
        assert_eq!(records[0].algorithm, "ssh-ed25519");
        assert_eq!(records[0].blob, b"blob");
    }

    /// 保存失败：challenge 保持未决，不降级为临时信任，错误结构化返回。
    #[test]
    fn save_failure_keeps_challenge_unresolved_without_temporary_trust() {
        use std::os::unix::fs::PermissionsExt;

        let app = mock_app();
        let events = Arc::new(AtomicUsize::new(0));
        let counter = events.clone();
        app.listen("host-identity:challenge", move |_| {
            counter.fetch_add(1, Ordering::Relaxed);
        });
        let (service, path) = service_with_record(TrustRecord {
            host: "10.0.0.9".to_string(),
            port: 22,
            algorithm: "ssh-ed25519".to_string(),
            blob: b"unrelated".to_vec(),
        });
        let verifier = service.verifier(app.handle().clone(), "session-1".to_string());
        let presented = make_presented("SHA256:fail");
        let waiter = thread::spawn({
            let presented = presented.clone();
            let verifier = verifier.clone();
            move || verifier(&presented)
        });
        let challenge = wait_pending(&service, "session-1");

        // 禁止父目录创建临时发布文件，但保留 known_hosts 可读取，使第二个 verifier
        // 仍能合并到原 challenge 并验证「保存失败不授予临时信任」。
        let parent = path.parent().expect("信任文件应有父目录");
        let original_permissions = fs::metadata(parent).unwrap().permissions();
        let mut read_only_permissions = original_permissions.clone();
        read_only_permissions.set_mode(0o555);
        fs::set_permissions(parent, read_only_permissions).unwrap();
        let error = service
            .accept_and_save(app.handle(), &challenge.challenge_id)
            .unwrap_err();
        fs::set_permissions(parent, original_permissions).unwrap();
        assert_eq!(error.code(), "HostKeySaveFailed");
        // challenge 保持未决：等待者仍在等待，pending 未清除
        assert_eq!(
            service.pending_challenge("session-1").unwrap().challenge_id,
            challenge.challenge_id
        );
        // 不降级为临时信任：同一 Session 的并发连接合并到同一 challenge，而非放行
        let verifier2 = service.verifier(app.handle().clone(), "session-1".to_string());
        let presented2 = presented.clone();
        let waiter2 = thread::spawn(move || verifier2(&presented2));
        let deadline = Instant::now() + Duration::from_secs(2);
        while service.waiting_connections(&challenge.challenge_id) < 2 && Instant::now() < deadline
        {
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            service.waiting_connections(&challenge.challenge_id),
            2,
            "保存失败不得授予临时信任"
        );
        assert_eq!(
            events.load(Ordering::Relaxed),
            1,
            "合并到同一 challenge 不重复派发"
        );
        // 用户可改选仅本次接受：challenge 正常解决，全部等待者放行
        service.accept(&challenge.challenge_id).unwrap();
        waiter.join().unwrap().expect("改选仅本次接受后放行");
        waiter2.join().unwrap().expect("改选仅本次接受后放行");
    }

    /// 应用 setup 路径：init_trust_store 后保存记录可读，未初始化则保存失败（fail-closed）。
    #[test]
    fn init_trust_store_populates_managed_store() {
        let app = mock_app();
        let service = HostIdentityService::new();
        service
            .init_trust_store(app.handle())
            .expect("初始化应成功");
        // mock app 的应用数据目录在测试间共享：使用唯一 host 避免测试间写入互相干扰
        let unique_host = format!("10.0.0.{}", &Uuid::new_v4().simple().to_string()[..8]);
        let presented = PresentedHostKey {
            host: unique_host.clone(),
            ..make_presented("SHA256:init")
        };
        let verifier = service.verifier(app.handle().clone(), "session-1".to_string());
        let waiter = thread::spawn(move || verifier(&presented));
        let challenge = wait_pending(&service, "session-1");
        service
            .accept_and_save(app.handle(), &challenge.challenge_id)
            .unwrap();
        waiter.join().unwrap().unwrap();
        // 初始化后的持久化信任生效：新 Session 同 endpoint 静默放行
        let verifier_b = service.verifier(app.handle().clone(), "session-b".to_string());
        verifier_b(&PresentedHostKey {
            host: unique_host,
            ..make_presented("SHA256:init")
        })
        .expect("初始化后的信任存储应命中");
    }

    /// 未初始化信任存储时保存失败且 challenge 保持未决（fail-closed，不吞错）。
    #[test]
    fn accept_and_save_without_store_keeps_challenge_pending() {
        let app = mock_app();
        let service = HostIdentityService::new();
        let verifier = service.verifier(app.handle().clone(), "session-1".to_string());
        let presented = make_presented("SHA256:nostore");
        let waiter = thread::spawn(move || verifier(&presented));
        let challenge = wait_pending(&service, "session-1");

        let error = service
            .accept_and_save(app.handle(), &challenge.challenge_id)
            .unwrap_err();
        assert_eq!(error.code(), "HostKeySaveFailed");
        assert_eq!(
            service.pending_challenge("session-1").unwrap().challenge_id,
            challenge.challenge_id
        );
        service.accept(&challenge.challenge_id).unwrap();
        waiter.join().unwrap().unwrap();
    }

    /// 跨 Session：保存成功后，其他 Session 中同 endpoint + 同 key 的 pending challenge 一并放行。
    #[test]
    fn save_releases_identical_pending_challenges_in_other_sessions() {
        let app = mock_app();
        let (service, _path) = service_with_record(TrustRecord {
            host: "10.0.0.9".to_string(),
            port: 22,
            algorithm: "ssh-ed25519".to_string(),
            blob: b"unrelated".to_vec(),
        });
        let verifier_a = service.verifier(app.handle().clone(), "session-a".to_string());
        let verifier_b = service.verifier(app.handle().clone(), "session-b".to_string());
        let presented = make_presented("SHA256:shared");
        let waiter_a = thread::spawn({
            let presented = presented.clone();
            let verifier_a = verifier_a.clone();
            move || verifier_a(&presented)
        });
        let waiter_b = thread::spawn(move || verifier_b(&presented));
        let challenge_a = wait_pending(&service, "session-a");
        let challenge_b = wait_pending(&service, "session-b");
        assert_ne!(challenge_a.challenge_id, challenge_b.challenge_id);

        service
            .accept_and_save(app.handle(), &challenge_a.challenge_id)
            .unwrap();
        waiter_a.join().unwrap().expect("发起保存的 Session 放行");
        waiter_b
            .join()
            .unwrap()
            .expect("相同 endpoint+key 的其他 Session 一并放行");
        assert!(service.pending_challenge("session-b").is_none());
    }

    /// 跨 Session 保存不放行不同 key 的 pending challenge；其等待者仍按需解决。
    #[test]
    fn save_does_not_release_pending_challenge_with_different_key() {
        let app = mock_app();
        let (service, _path) = service_with_record(TrustRecord {
            host: "10.0.0.9".to_string(),
            port: 22,
            algorithm: "ssh-ed25519".to_string(),
            blob: b"unrelated".to_vec(),
        });
        let verifier_a = service.verifier(app.handle().clone(), "session-a".to_string());
        let verifier_c = service.verifier(app.handle().clone(), "session-c".to_string());
        let presented_a = make_presented("SHA256:saved-key");
        let presented_c = PresentedHostKey {
            blob: b"different".to_vec(),
            ..make_presented("SHA256:other-key")
        };
        let waiter_a = thread::spawn(move || verifier_a(&presented_a));
        let waiter_c = thread::spawn(move || verifier_c(&presented_c));
        let challenge_a = wait_pending(&service, "session-a");
        let challenge_c = wait_pending(&service, "session-c");

        service
            .accept_and_save(app.handle(), &challenge_a.challenge_id)
            .unwrap();
        waiter_a.join().unwrap().unwrap();
        // session-c 的 key 不同：保存不自动放行，challenge 仍待用户决定
        assert_eq!(
            service.pending_challenge("session-c").unwrap().challenge_id,
            challenge_c.challenge_id
        );
        service.accept(&challenge_c.challenge_id).unwrap();
        waiter_c.join().unwrap().unwrap();
    }

    /// 持久化记录的放行/挑战决定必须在最终 state 锁内做出：
    /// 查找若发生在锁外，并发 accept_and_save（他 Session 刚保存本 key）会在
    /// 查找与决定之间留下 stale kind 竞态窗口。用 FIFO 阻塞 lookup，观察 lookup
    /// 期间 state 锁被持有（记录决定与 challenge 创建在同一临界区内），
    /// 且 lookup 期间才落盘的匹配记录仍被采用（静默放行，不弹挑战）。
    #[test]
    fn persisted_record_decision_happens_under_final_state_lock() {
        use std::io::Write as _;
        use std::os::unix::fs::OpenOptionsExt;
        use std::process::Command;

        let dir = std::env::temp_dir().join(format!("titan-identity-fifo-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).expect("temp dir should be created");
        let fifo = dir.join("known_hosts");
        assert!(
            Command::new("mkfifo")
                .arg(&fifo)
                .status()
                .expect("应能创建 FIFO")
                .success()
        );

        let app = mock_app();
        // 缓存未加载：lookup 触发文件读取并阻塞在 FIFO 上
        let service = HostIdentityService::with_trust_store_path(fifo.clone());
        let presented = make_presented("SHA256:match");
        let verifier = service.verifier(app.handle().clone(), "session-1".to_string());
        let handle = thread::spawn({
            let verifier = verifier.clone();
            let presented = presented.clone();
            move || verifier(&presented)
        });

        // 非阻塞 writer 探测 lookup 阻塞（reader 就位）：此时 state 锁必须被持有，
        // 否则记录决定发生在锁外，存在 stale kind 竞态窗口
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut probe_writer = None;
        while Instant::now() < deadline {
            match fs::OpenOptions::new()
                .write(true)
                .custom_flags(0x800)
                .open(&fifo)
            {
                Ok(writer) => {
                    probe_writer = Some(writer);
                    assert!(
                        service.state.try_lock().is_err(),
                        "持久化记录决定必须发生在最终 state 锁内（lookup 期间 state 锁应被持有）"
                    );
                    break;
                }
                Err(_) => thread::sleep(Duration::from_millis(10)),
            }
        }
        let mut writer = probe_writer.expect("verify 应在期限内进入 lookup");

        // lookup 期间才落盘的精确匹配记录也必须生效：写入匹配记录后
        // verify 应静默放行，不因过期快照把用户多阻塞一次
        writer
            .write_all(b"10.0.0.8 ssh-ed25519 YmxvYg\n")
            .expect("应能写入记录");
        drop(writer);
        handle
            .join()
            .expect("verify 线程不应 panic")
            .expect("匹配记录应放行");
        assert!(service.pending_challenge("session-1").is_none());
        assert!(service.is_trusted("session-1", "10.0.0.8", 22, "SHA256:match"));
        let _ = fs::remove_dir_all(&dir);
    }

    /// pending_index 与 pending 脱同步（历史 bug / 未来重构残留）时不得在
    /// 传输线程上 panic：按未命中处理重新创建 challenge，校验继续正常完成。
    #[test]
    fn desynced_pending_index_falls_back_to_new_challenge() {
        let app = mock_app();
        let service = HostIdentityService::new();
        let key = IdentityKey {
            session_id: "session-1".to_string(),
            host: "10.0.0.8".to_string(),
            port: 22,
            fingerprint: "SHA256:desync".to_string(),
        };
        // 人为制造索引指向不存在 challenge 的脱同步状态
        {
            let mut state = service.state.lock().unwrap();
            state
                .pending_index
                .insert(key.clone(), "ghost-challenge".to_string());
        }
        let verifier = service.verifier(app.handle().clone(), "session-1".to_string());
        let presented = make_presented("SHA256:desync");
        let waiter = thread::spawn({
            let verifier = verifier.clone();
            let presented = presented.clone();
            move || verifier(&presented)
        });
        let challenge = wait_pending(&service, "session-1");
        assert_ne!(
            challenge.challenge_id, "ghost-challenge",
            "脱同步索引必须按未命中重新创建 challenge"
        );
        // 创建路径以新 challenge 覆盖残留索引条目
        {
            let state = service.state.lock().unwrap();
            assert_eq!(
                state.pending_index.get(&key),
                Some(&challenge.challenge_id),
                "新 challenge 应覆盖残留索引"
            );
        }
        service.accept(&challenge.challenge_id).unwrap();
        waiter.join().unwrap().expect("脱同步索引不得使校验 panic");
    }

    /// 事件派发失败（webview 已销毁、runtime 错误等）不得静默吞掉：
    /// challenge 必须移除，全部等待者立即以 HostKeyVerificationCancelled 失败，
    /// 连接不得在用户看不到的 challenge 上永久阻塞。
    #[test]
    fn emit_failure_cancels_challenge_and_wakes_all_waiters() {
        let app = mock_app();
        let service = HostIdentityService::new();
        let presented = make_presented("SHA256:noemit");
        let (gate_tx, gate_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();

        // creator：派发阻塞在门后并最终失败（challenge 已插入 pending）
        let creator = thread::spawn({
            let service = service.clone();
            let app_handle = app.handle().clone();
            let presented = presented.clone();
            move || {
                service.verify_with_emitter(
                    &app_handle,
                    "session-1",
                    &presented,
                    |_app, _challenge| {
                        gate_tx.send(()).expect("应能发送门信号");
                        release_rx.recv().expect("应能收到放行信号");
                        Err("mock 派发失败".to_string())
                    },
                )
            }
        });
        // 等待 creator 进入 emit（challenge 已创建，仍 pending）
        gate_rx.recv().expect("creator 应进入派发");

        // joiner：合并到同一 challenge 的并发连接（created=false，不再派发）
        let verifier = service.verifier(app.handle().clone(), "session-1".to_string());
        let joiner = thread::spawn({
            let verifier = verifier.clone();
            let presented = presented.clone();
            move || verifier(&presented)
        });
        let challenge = wait_pending(&service, "session-1");
        let deadline = Instant::now() + Duration::from_secs(2);
        while service.waiting_connections(&challenge.challenge_id) < 1 && Instant::now() < deadline
        {
            thread::sleep(Duration::from_millis(10));
        }
        assert!(
            service.waiting_connections(&challenge.challenge_id) >= 1,
            "并发连接应合并到同一 challenge"
        );

        // 放行派发失败：challenge 移除，两个等待者立即以取消错误失败
        release_tx.send(()).expect("应能放行派发");
        assert_eq!(
            creator.join().unwrap().unwrap_err().code(),
            "HostKeyVerificationCancelled"
        );
        assert_eq!(
            joiner.join().unwrap().unwrap_err().code(),
            "HostKeyVerificationCancelled"
        );
        assert!(
            service.pending_challenge("session-1").is_none(),
            "派发失败的 challenge 不得残留 pending"
        );
    }

    /// 等待并收集指定数量的撤销事件（期限内不足则 panic）。
    fn wait_dismissals(
        dismissals: &Arc<Mutex<Vec<HostIdentityChallengeDismissed>>>,
        count: usize,
    ) -> Vec<HostIdentityChallengeDismissed> {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let current = dismissals.lock().unwrap().clone();
            if current.len() >= count {
                return current;
            }
            assert!(Instant::now() < deadline, "撤销事件应在期限内到达");
            thread::sleep(Duration::from_millis(10));
        }
    }

    /// challenge 被新指纹取代时，旧 challenge 的确认卡必须收到撤销事件，
    /// UI 不得残留孤儿提示（其 accept/reject 只会得到 HostKeyChallengeNotFound）。
    #[test]
    fn superseded_challenge_emits_dismissal() {
        let app = mock_app();
        let dismissals = Arc::new(Mutex::new(Vec::new()));
        let captured = dismissals.clone();
        app.listen("host-identity:challenge-dismissed", move |event| {
            let payload: HostIdentityChallengeDismissed =
                serde_json::from_str(event.payload()).expect("payload 可反序列化");
            captured.lock().unwrap().push(payload);
        });
        let service = HostIdentityService::new();
        let verifier = service.verifier(app.handle().clone(), "session-1".to_string());
        let presented_a = make_presented("SHA256:aaa");
        let presented_b = PresentedHostKey {
            fingerprint: "SHA256:bbb".to_string(),
            ..presented_a.clone()
        };

        let waiter_a = thread::spawn({
            let verifier = verifier.clone();
            let presented = presented_a.clone();
            move || verifier(&presented)
        });
        let challenge_a = wait_pending(&service, "session-1");
        let waiter_b = thread::spawn({
            let verifier = verifier.clone();
            move || verifier(&presented_b)
        });
        let challenge_b = wait_pending_other(&service, "session-1", &challenge_a.challenge_id);

        // 旧 challenge 的确认卡必须收到撤销事件
        let dismissals = wait_dismissals(&dismissals, 1);
        assert_eq!(
            dismissals,
            vec![HostIdentityChallengeDismissed {
                challenge_id: challenge_a.challenge_id,
                session_id: "session-1".to_string(),
            }]
        );
        // 旧等待者取消，新 challenge 可正常解决
        assert_eq!(
            waiter_a.join().unwrap().unwrap_err().code(),
            "HostKeyVerificationCancelled"
        );
        service.accept(&challenge_b.challenge_id).unwrap();
        waiter_b.join().unwrap().expect("新 challenge 接受后放行");
    }

    /// accept_and_save 异地解决的其他 Session challenge 必须收到撤销事件；
    /// 发起保存的 challenge 由 UI 自行撤下，不重复派发。
    #[test]
    fn accept_and_save_dismisses_other_session_prompts() {
        let app = mock_app();
        let dismissals = Arc::new(Mutex::new(Vec::new()));
        let captured = dismissals.clone();
        app.listen("host-identity:challenge-dismissed", move |event| {
            let payload: HostIdentityChallengeDismissed =
                serde_json::from_str(event.payload()).expect("payload 可反序列化");
            captured.lock().unwrap().push(payload);
        });
        let (service, _path) = service_with_record(TrustRecord {
            host: "10.0.0.9".to_string(),
            port: 22,
            algorithm: "ssh-ed25519".to_string(),
            blob: b"unrelated".to_vec(),
        });
        let verifier_a = service.verifier(app.handle().clone(), "session-a".to_string());
        let verifier_b = service.verifier(app.handle().clone(), "session-b".to_string());
        let presented = make_presented("SHA256:shared");
        let waiter_a = thread::spawn({
            let verifier = verifier_a.clone();
            let presented = presented.clone();
            move || verifier(&presented)
        });
        let waiter_b = thread::spawn({
            let verifier = verifier_b.clone();
            let presented = presented.clone();
            move || verifier(&presented)
        });
        let challenge_a = wait_pending(&service, "session-a");
        let challenge_b = wait_pending(&service, "session-b");

        service
            .accept_and_save(app.handle(), &challenge_a.challenge_id)
            .unwrap();
        waiter_a.join().unwrap().expect("发起保存的 Session 放行");
        waiter_b
            .join()
            .unwrap()
            .expect("相同 endpoint+key 一并放行");

        let dismissals = wait_dismissals(&dismissals, 1);
        assert_eq!(
            dismissals,
            vec![HostIdentityChallengeDismissed {
                challenge_id: challenge_b.challenge_id,
                session_id: "session-b".to_string(),
            }]
        );
    }

    /// cancel_session 移除的未决 challenge 必须收到撤销事件，UI 不得残留孤儿提示。
    #[test]
    fn cancel_session_dismisses_pending_prompts() {
        let app = mock_app();
        let dismissals = Arc::new(Mutex::new(Vec::new()));
        let captured = dismissals.clone();
        app.listen("host-identity:challenge-dismissed", move |event| {
            let payload: HostIdentityChallengeDismissed =
                serde_json::from_str(event.payload()).expect("payload 可反序列化");
            captured.lock().unwrap().push(payload);
        });
        let service = HostIdentityService::new();
        let verifier = service.verifier(app.handle().clone(), "session-1".to_string());
        let presented = make_presented("SHA256:cancel");
        let waiter = thread::spawn({
            let verifier = verifier.clone();
            let presented = presented.clone();
            move || verifier(&presented)
        });
        let challenge = wait_pending(&service, "session-1");

        service.cancel_session(app.handle(), "session-1");
        assert_eq!(
            waiter.join().unwrap().unwrap_err().code(),
            "HostKeyVerificationCancelled"
        );
        assert_eq!(
            wait_dismissals(&dismissals, 1),
            vec![HostIdentityChallengeDismissed {
                challenge_id: challenge.challenge_id,
                session_id: "session-1".to_string(),
            }]
        );
    }

    /// cancel_all 移除的全部未决 challenge 必须收到撤销事件。
    #[test]
    fn cancel_all_dismisses_all_pending_prompts() {
        let app = mock_app();
        let dismissals = Arc::new(Mutex::new(Vec::new()));
        let captured = dismissals.clone();
        app.listen("host-identity:challenge-dismissed", move |event| {
            let payload: HostIdentityChallengeDismissed =
                serde_json::from_str(event.payload()).expect("payload 可反序列化");
            captured.lock().unwrap().push(payload);
        });
        let service = HostIdentityService::new();
        let verifier_a = service.verifier(app.handle().clone(), "session-a".to_string());
        let verifier_b = service.verifier(app.handle().clone(), "session-b".to_string());
        let waiters: Vec<_> = [
            ("session-a", verifier_a.clone()),
            ("session-b", verifier_b.clone()),
        ]
        .iter()
        .map(|(session_id, verifier)| {
            let verifier = verifier.clone();
            let presented = make_presented(&format!("SHA256:exit-{session_id}"));
            thread::spawn(move || verifier(&presented))
        })
        .collect();
        let deadline = Instant::now() + Duration::from_secs(2);
        while (service.pending_challenge("session-a").is_none()
            || service.pending_challenge("session-b").is_none())
            && Instant::now() < deadline
        {
            thread::sleep(Duration::from_millis(10));
        }

        service.cancel_all(app.handle());
        for waiter in waiters {
            assert_eq!(
                waiter.join().unwrap().unwrap_err().code(),
                "HostKeyVerificationCancelled"
            );
        }
        let mut dismissals = wait_dismissals(&dismissals, 2);
        dismissals.sort_by(|a, b| a.session_id.cmp(&b.session_id));
        assert_eq!(
            dismissals
                .iter()
                .map(|d| d.session_id.as_str())
                .collect::<Vec<_>>(),
            vec!["session-a", "session-b"]
        );
        assert!(
            dismissals.iter().all(|d| !d.challenge_id.is_empty()),
            "撤销事件必须携带 challenge id"
        );
    }

    /// 首次未知主机产生 challenge 事件；接受后同一 Session 的后续连接（含重连）直接放行。
    #[test]
    fn accept_once_allows_subsequent_connections_in_same_session() {
        let app = mock_app();
        let service = HostIdentityService::new();
        let events = Arc::new(AtomicUsize::new(0));
        let counter = events.clone();
        app.listen("host-identity:challenge", move |_| {
            counter.fetch_add(1, Ordering::Relaxed);
        });

        let verifier = service.verifier(app.handle().clone(), "session-1".to_string());
        let presented = make_presented("SHA256:aaa");

        // 首次：阻塞等待用户决定
        let wait_verifier = verifier.clone();
        let presented_clone = presented.clone();
        let waiter = thread::spawn(move || wait_verifier(&presented_clone));
        let deadline = Instant::now() + Duration::from_secs(2);
        while service.pending_challenge("session-1").is_none() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        let challenge = service
            .pending_challenge("session-1")
            .expect("challenge 已创建");

        service.accept(&challenge.challenge_id).unwrap();
        waiter.join().unwrap().expect("接受后放行");

        // 第二次连接（模拟 capability reconnect）：已信任，直接放行且不产生新 challenge
        verifier(&presented).expect("同一 Session 内已信任");
        assert_eq!(events.load(Ordering::Relaxed), 1);
    }

    /// 信任以 Runtime Session 为作用域：其他 Session 连接同一 endpoint 仍需确认。
    #[test]
    fn trust_is_scoped_to_runtime_session() {
        let app = mock_app();
        let service = HostIdentityService::new();
        let verifier_a = service.verifier(app.handle().clone(), "session-a".to_string());
        let presented = make_presented("SHA256:aaa");

        // session-a 后台等待确认
        let v = verifier_a.clone();
        let p = presented.clone();
        let waiter = thread::spawn(move || v(&p));
        let deadline = Instant::now() + Duration::from_secs(2);
        while service.pending_challenge("session-a").is_none() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        let challenge = service.pending_challenge("session-a").unwrap();
        service.accept(&challenge.challenge_id).unwrap();
        waiter.join().unwrap().unwrap();

        // session-b 连接同一 endpoint+指纹：不受 session-a 的信任影响，产生新 challenge
        let verifier_b = service.verifier(app.handle().clone(), "session-b".to_string());
        let v = verifier_b.clone();
        let p = presented.clone();
        let waiter_b = thread::spawn(move || v(&p));
        let deadline = Instant::now() + Duration::from_secs(2);
        while service.pending_challenge("session-b").is_none() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        let challenge_b = service
            .pending_challenge("session-b")
            .expect("session-b 独立确认");
        service.reject(&challenge_b.challenge_id).unwrap();
        let error = waiter_b.join().unwrap().unwrap_err();
        assert_eq!(error.code(), "HostKeyRejected");
    }

    /// 同一 Session、endpoint 与指纹的并发连接合并为一个 challenge；接受后全部放行。
    #[test]
    fn concurrent_connections_merge_into_single_challenge() {
        let app = mock_app();
        let service = HostIdentityService::new();
        let events = Arc::new(AtomicUsize::new(0));
        let counter = events.clone();
        app.listen("host-identity:challenge", move |_| {
            counter.fetch_add(1, Ordering::Relaxed);
        });

        let verifier = service.verifier(app.handle().clone(), "session-1".to_string());
        let presented = make_presented("SHA256:merge");

        let waiters: Vec<_> = (0..4)
            .map(|_| {
                let v = verifier.clone();
                let p = presented.clone();
                thread::spawn(move || v(&p))
            })
            .collect();

        // 等待 challenge 出现；多个并发等待者只产生一个 challenge
        let deadline = Instant::now() + Duration::from_secs(2);
        while service.pending_challenge("session-1").is_none() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        thread::sleep(Duration::from_millis(50));
        let challenge = service
            .pending_challenge("session-1")
            .expect("challenge 已创建");

        service.accept(&challenge.challenge_id).unwrap();
        for waiter in waiters {
            waiter.join().unwrap().expect("全部等待者接受后继续");
        }
        assert_eq!(
            events.load(Ordering::Relaxed),
            1,
            "并发连接合并为一个 challenge"
        );
    }

    /// 拒绝后全部等待者以 HostKeyRejected 失败，不进入认证。
    #[test]
    fn reject_fails_all_waiters() {
        let app = mock_app();
        let service = HostIdentityService::new();
        let verifier = service.verifier(app.handle().clone(), "session-1".to_string());
        let presented = make_presented("SHA256:deny");

        let waiters: Vec<_> = (0..3)
            .map(|_| {
                let v = verifier.clone();
                let p = presented.clone();
                thread::spawn(move || v(&p))
            })
            .collect();
        let deadline = Instant::now() + Duration::from_secs(2);
        while service.pending_challenge("session-1").is_none() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        let challenge = service.pending_challenge("session-1").unwrap();
        // 等待全部并发连接合并到同一 challenge 后再拒绝，避免迟到连接另建 challenge
        while service.waiting_connections(&challenge.challenge_id) < 3 {
            assert!(
                Instant::now() < deadline,
                "并发连接应在超时前合并到同一 challenge"
            );
            thread::sleep(Duration::from_millis(10));
        }

        let rejected = service.reject(&challenge.challenge_id).unwrap();
        assert_eq!(rejected.session_id, "session-1");
        for waiter in waiters {
            let error = waiter.join().unwrap().unwrap_err();
            assert_eq!(error.code(), "HostKeyRejected");
        }
        // 拒绝不写入信任：新连接产生新 challenge 而非静默放行
        let v = verifier.clone();
        let p = presented.clone();
        let retry = thread::spawn(move || v(&p));
        let deadline = Instant::now() + Duration::from_secs(2);
        while service.pending_challenge("session-1").is_none() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        let retried = service
            .pending_challenge("session-1")
            .expect("拒绝后新连接重新确认");
        service.reject(&retried.challenge_id).unwrap();
        assert_eq!(retry.join().unwrap().unwrap_err().code(), "HostKeyRejected");
    }

    /// 会话关闭取消全部等待者并清除临时信任；等待者以取消错误退出且不再阻塞。
    #[test]
    fn cancel_session_waits_no_more_and_clears_trust() {
        let app = mock_app();
        let service = HostIdentityService::new();
        let verifier = service.verifier(app.handle().clone(), "session-1".to_string());
        let presented = make_presented("SHA256:cancel");

        let v = verifier.clone();
        let p = presented.clone();
        let waiter = thread::spawn(move || v(&p));
        let deadline = Instant::now() + Duration::from_secs(2);
        while service.pending_challenge("session-1").is_none() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }

        service.cancel_session(app.handle(), "session-1");
        let error = waiter.join().unwrap().unwrap_err();
        assert_eq!(error.code(), "HostKeyVerificationCancelled");
        assert!(service.pending_challenge("session-1").is_none());

        // 清除临时信任：另一 Session 先接受后取消，信任必须被移除（直接观察状态，
        // 因为取消后的 Session 校验器按设计直接失败，不会再产生 challenge）
        let verifier_b = service.verifier(app.handle().clone(), "session-2".to_string());
        let v = verifier_b.clone();
        let p = presented.clone();
        let waiter = thread::spawn(move || v(&p));
        let deadline = Instant::now() + Duration::from_secs(2);
        while service.pending_challenge("session-2").is_none() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        let challenge = service
            .pending_challenge("session-2")
            .expect("session-2 产生 challenge");
        service.accept(&challenge.challenge_id).unwrap();
        waiter.join().unwrap().expect("接受后放行");
        assert!(
            service.is_trusted("session-2", "10.0.0.8", 22, "SHA256:cancel"),
            "接受后写入临时信任"
        );
        service.cancel_session(app.handle(), "session-2");
        assert!(
            !service.is_trusted("session-2", "10.0.0.8", 22, "SHA256:cancel"),
            "Session 关闭必须清除临时信任"
        );
    }

    /// 关闭后的 Session 上迟到到达的校验器（如已发放给 Monitoring worker）不得再
    /// 创建无人取消的 challenge：verify 立即以取消错误返回，等待者不会永久阻塞。
    #[test]
    fn cancelled_session_verifier_fails_fast_without_new_challenge() {
        let app = mock_app();
        let service = HostIdentityService::new();
        let verifier = service.verifier(app.handle().clone(), "session-gone".to_string());

        service.cancel_session(app.handle(), "session-gone");
        let error = verifier(&make_presented("SHA256:late")).unwrap_err();
        assert_eq!(error.code(), "HostKeyVerificationCancelled");
        assert!(
            service.pending_challenge("session-gone").is_none(),
            "取消后的 Session 不得产生新的 pending challenge"
        );
    }

    /// 应用退出路径：cancel_all 唤醒全部 Session 的全部等待者，pending 清空。
    #[test]
    fn cancel_all_wakes_all_waiters() {
        let app = mock_app();
        let service = HostIdentityService::new();
        let verifier_a = service.verifier(app.handle().clone(), "session-a".to_string());
        let verifier_b = service.verifier(app.handle().clone(), "session-b".to_string());

        let waiters: Vec<_> = [("session-a", &verifier_a), ("session-b", &verifier_b)]
            .iter()
            .map(|(session_id, verifier)| {
                let v = (*verifier).clone();
                let p = make_presented(&format!("SHA256:exit-{session_id}"));
                thread::spawn(move || v(&p))
            })
            .collect();
        let deadline = Instant::now() + Duration::from_secs(2);
        while (service.pending_challenge("session-a").is_none()
            || service.pending_challenge("session-b").is_none())
            && Instant::now() < deadline
        {
            thread::sleep(Duration::from_millis(10));
        }

        service.cancel_all(app.handle());
        for waiter in waiters {
            let error = waiter.join().unwrap().unwrap_err();
            assert_eq!(error.code(), "HostKeyVerificationCancelled");
        }
        assert!(service.pending_challenge("session-a").is_none());
        assert!(service.pending_challenge("session-b").is_none());
    }

    /// accept/reject 不存在的 challenge 返回稳定错误。
    #[test]
    fn unknown_challenge_returns_stable_error() {
        let service = HostIdentityService::new();
        assert_eq!(
            service.accept("missing").unwrap_err().code(),
            "HostKeyChallengeNotFound"
        );
        assert_eq!(
            service.reject("missing").unwrap_err().code(),
            "HostKeyChallengeNotFound"
        );
    }

    /// challenge 事件 payload 为 camelCase 且字段完整（前端不解析 SSH key 文本）；
    /// Unknown 与 Changed 的 kind、stored 字段均正确序列化。
    #[test]
    fn challenge_event_serializes_as_camel_case_payload() {
        let unknown = HostIdentityChallenge {
            challenge_id: "c-1".to_string(),
            session_id: "session-1".to_string(),
            host: "10.0.0.8".to_string(),
            port: 22,
            kind: HostIdentityChallengeKind::Unknown,
            key_algorithm: "ssh-ed25519".to_string(),
            fingerprint: "SHA256:aaa".to_string(),
            stored_algorithm: None,
            stored_fingerprint: None,
            timestamp: 1_710_000_000_000,
        };
        let value = serde_json::to_value(&unknown).unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "challengeId": "c-1",
                "sessionId": "session-1",
                "host": "10.0.0.8",
                "port": 22,
                "kind": "Unknown",
                "keyAlgorithm": "ssh-ed25519",
                "fingerprint": "SHA256:aaa",
                "storedAlgorithm": null,
                "storedFingerprint": null,
                "timestamp": 1_710_000_000_000_i64
            })
        );
        let changed = HostIdentityChallenge {
            challenge_id: "c-2".to_string(),
            kind: HostIdentityChallengeKind::Changed,
            key_algorithm: "ssh-rsa".to_string(),
            fingerprint: "SHA256:bbb".to_string(),
            stored_algorithm: Some("ssh-ed25519".to_string()),
            stored_fingerprint: Some(fingerprint_sha256(b"old")),
            ..unknown
        };
        let value = serde_json::to_value(&changed).unwrap();
        assert_eq!(value["kind"], "Changed");
        assert_eq!(value["storedAlgorithm"], "ssh-ed25519");
        assert_eq!(value["storedFingerprint"], fingerprint_sha256(b"old"));
        let _: Value = value;
    }

    /// 指纹使用 OpenSSH 已知向量：SHA-256("abc") 的 base64 无填充。
    #[test]
    fn fingerprint_matches_known_vector() {
        assert_eq!(
            fingerprint_sha256(b"abc"),
            "SHA256:ungWv48Bz+pBQUDeXa4iI7ADYaOWF3qctBD/YfIAFa0"
        );
    }

    /// 主机密钥算法名映射为 OpenSSH 风格。
    #[test]
    fn algorithm_names_follow_openssh_style() {
        assert_eq!(algorithm_name(HostKeyType::Ed25519), "ssh-ed25519");
        assert_eq!(algorithm_name(HostKeyType::Rsa), "ssh-rsa");
        assert_eq!(algorithm_name(HostKeyType::Ecdsa256), "ecdsa-sha2-nistp256");
        assert_eq!(algorithm_name(HostKeyType::Unknown), "unknown");
    }

    /// 生命周期清理移除持久化记录后，新 Session 将 endpoint 视为未知并重新确认。
    #[test]
    fn forget_endpoint_removes_persisted_record_and_new_sessions_prompt() {
        let app = mock_app();
        let (service, path) = service_with_record(TrustRecord {
            host: "10.0.0.8".to_string(),
            port: 22,
            algorithm: "ssh-ed25519".to_string(),
            blob: b"blob".to_vec(),
        });

        service.forget_endpoint("10.0.0.8", 22).unwrap();
        // 持久化记录已删除（重新构造 TrustStore 绕过内存缓存观察磁盘真相）
        assert_eq!(
            TrustStore::from_file_path(path)
                .lookup("10.0.0.8", 22)
                .unwrap(),
            None
        );
        // 新 Session 重新视为未知：产生 challenge 等待确认
        let verifier = service.verifier(app.handle().clone(), "session-1".to_string());
        let presented = make_presented("SHA256:again");
        let waiter = thread::spawn(move || verifier(&presented));
        let challenge = wait_pending(&service, "session-1");
        service.reject(&challenge.challenge_id).unwrap();
        assert_eq!(
            waiter.join().unwrap().unwrap_err().code(),
            "HostKeyRejected"
        );
    }

    /// 清理不干扰运行中的 Runtime Session：已持有的临时信任持续到 Session 关闭，
    /// 持久化记录移除后同 Session 重连仍放行。
    #[test]
    fn forget_endpoint_keeps_active_session_temporary_trust() {
        let app = mock_app();
        let (service, _path) = service_with_record(TrustRecord {
            host: "10.0.0.8".to_string(),
            port: 22,
            algorithm: "ssh-ed25519".to_string(),
            blob: b"persisted".to_vec(),
        });
        // session-1 呈现与持久化不同的 key：challenge → 仅本次接受（临时信任）
        let presented = PresentedHostKey {
            blob: b"rotated".to_vec(),
            ..make_presented("SHA256:rotated")
        };
        let verifier = service.verifier(app.handle().clone(), "session-1".to_string());
        let waiter = thread::spawn({
            let verifier = verifier.clone();
            let presented = presented.clone();
            move || verifier(&presented)
        });
        let challenge = wait_pending(&service, "session-1");
        service.accept(&challenge.challenge_id).unwrap();
        waiter.join().unwrap().unwrap();

        // 生命周期清理移除持久化记录：不影响运行中的 Session
        service.forget_endpoint("10.0.0.8", 22).unwrap();

        // 同 Session 重连仍放行（临时信任持续到关闭）
        verifier(&presented).expect("活动 Session 的临时信任不受清理影响");
        // 新 Session 视为未知：重新触发确认
        let verifier_b = service.verifier(app.handle().clone(), "session-b".to_string());
        let waiter_b = thread::spawn({
            let verifier = verifier_b.clone();
            let presented = presented.clone();
            move || verifier(&presented)
        });
        let challenge_b = wait_pending(&service, "session-b");
        service.reject(&challenge_b.challenge_id).unwrap();
        assert_eq!(
            waiter_b.join().unwrap().unwrap_err().code(),
            "HostKeyRejected"
        );
    }

    /// 已通过持久化匹配静默验证的 Session：清理后同 Session 重连仍静默放行
    /// （已验证决定持续到 Session 关闭），新 Session 重新确认。
    #[test]
    fn forget_endpoint_keeps_silently_verified_session_decision() {
        let app = mock_app();
        let (service, _path) = service_with_record(TrustRecord {
            host: "10.0.0.8".to_string(),
            port: 22,
            algorithm: "ssh-ed25519".to_string(),
            blob: b"blob".to_vec(),
        });
        let verifier = service.verifier(app.handle().clone(), "session-1".to_string());
        let presented = make_presented("SHA256:match");
        verifier(&presented).expect("持久化匹配静默放行");

        // 生命周期清理移除持久化记录
        service.forget_endpoint("10.0.0.8", 22).unwrap();
        // 同 Session 重连：已验证决定持续到 Session 关闭，不产生新 challenge
        verifier(&presented).expect("已验证决定持续到 Session 关闭");
        assert!(service.pending_challenge("session-1").is_none());
        // 新 Session 视为未知并重新确认
        let verifier_b = service.verifier(app.handle().clone(), "session-b".to_string());
        let waiter_b = thread::spawn(move || verifier_b(&presented));
        let challenge_b = wait_pending(&service, "session-b");
        service.reject(&challenge_b.challenge_id).unwrap();
        assert_eq!(
            waiter_b.join().unwrap().unwrap_err().code(),
            "HostKeyRejected"
        );
    }

    /// 移除不存在的 endpoint 幂等成功（HostConfig 从未受信任的 endpoint 编辑路径）。
    #[test]
    fn forget_endpoint_missing_endpoint_is_idempotent() {
        let (service, _path) = service_with_record(TrustRecord {
            host: "10.0.0.8".to_string(),
            port: 22,
            algorithm: "ssh-ed25519".to_string(),
            blob: b"blob".to_vec(),
        });
        service.forget_endpoint("10.0.0.9", 2222).unwrap();
        service.forget_endpoint("10.0.0.8", 22).unwrap();
    }

    /// 信任存储未初始化时清理 fail-closed：显式报错，不得静默吞掉未完成的清理。
    #[test]
    fn forget_endpoint_without_store_fails_closed() {
        let service = HostIdentityService::new();
        let error = service.forget_endpoint("10.0.0.8", 22).unwrap_err();
        assert_eq!(error.code(), "TrustStoreError");
    }

    /// 等待指定 Session 出现与给定 id 不同的 pending challenge（旧 challenge 被取代后
    /// 新 challenge 出现）；超时则 panic。
    fn wait_pending_other(
        service: &HostIdentityService,
        session_id: &str,
        exclude: &str,
    ) -> HostIdentityChallenge {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if let Some(challenge) = service.pending_challenge(session_id)
                && challenge.challenge_id != exclude
            {
                return challenge;
            }
            assert!(Instant::now() < deadline, "新 challenge 应在超时前创建");
            thread::sleep(Duration::from_millis(10));
        }
    }

    /// 已保存 key 与呈现不一致：产生 Changed challenge，携带旧记录与新呈现的算法/指纹，
    /// 不覆盖或删除旧记录，也不开始认证；替换成功后 endpoint 只保留呈现 key。
    #[test]
    fn changed_key_produces_changed_challenge_with_stored_and_presented_info() {
        let app = mock_app();
        let payloads = Arc::new(Mutex::new(Vec::new()));
        let captured = payloads.clone();
        app.listen("host-identity:challenge", move |event| {
            let payload: HostIdentityChallenge =
                serde_json::from_str(event.payload()).expect("payload 可反序列化");
            captured.lock().unwrap().push(payload);
        });
        let (service, path) = service_with_record(TrustRecord {
            host: "10.0.0.8".to_string(),
            port: 22,
            algorithm: "ssh-ed25519".to_string(),
            blob: b"old-blob".to_vec(),
        });
        let verifier = service.verifier(app.handle().clone(), "session-1".to_string());
        let presented = PresentedHostKey {
            algorithm: "ssh-rsa".to_string(),
            blob: b"new-blob".to_vec(),
            ..make_presented("SHA256:presented")
        };
        let waiter = thread::spawn({
            let verifier = verifier.clone();
            let presented = presented.clone();
            move || verifier(&presented)
        });
        let challenge = wait_pending(&service, "session-1");

        // Changed challenge 同时展示旧记录与新呈现的算法/指纹
        assert_eq!(challenge.kind, HostIdentityChallengeKind::Changed);
        assert_eq!(challenge.key_algorithm, "ssh-rsa");
        assert_eq!(challenge.fingerprint, "SHA256:presented");
        assert_eq!(challenge.stored_algorithm.as_deref(), Some("ssh-ed25519"));
        assert_eq!(
            challenge.stored_fingerprint.as_deref(),
            Some(fingerprint_sha256(b"old-blob").as_str())
        );
        // 事件 payload 同样携带 kind 与新旧信息
        let payloads = payloads.lock().unwrap();
        assert_eq!(payloads.len(), 1);
        assert_eq!(payloads[0].kind, HostIdentityChallengeKind::Changed);
        assert_eq!(payloads[0].stored_algorithm.as_deref(), Some("ssh-ed25519"));
        drop(payloads);

        // 旧记录未被覆盖或删除（磁盘真相），认证未开始（等待者仍在等待）
        let records = TrustStore::from_file_path(path.clone()).reload().unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].algorithm, "ssh-ed25519");
        assert_eq!(records[0].blob, b"old-blob");
        assert_eq!(service.waiting_connections(&challenge.challenge_id), 1);

        // 替换成功：endpoint 只保留呈现 key
        service
            .accept_and_save(app.handle(), &challenge.challenge_id)
            .unwrap();
        waiter.join().unwrap().expect("替换成功后放行认证");
        let records = TrustStore::from_file_path(path).reload().unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].algorithm, "ssh-rsa");
        assert_eq!(records[0].blob, b"new-blob");
    }

    /// 仅算法变化（公钥材料相同）同样产生 Changed challenge，不静默放行。
    #[test]
    fn algorithm_change_alone_produces_changed_challenge() {
        let app = mock_app();
        let (service, _path) = service_with_record(TrustRecord {
            host: "10.0.0.8".to_string(),
            port: 22,
            algorithm: "ssh-rsa".to_string(),
            blob: b"same-blob".to_vec(),
        });
        let verifier = service.verifier(app.handle().clone(), "session-1".to_string());
        let presented = PresentedHostKey {
            blob: b"same-blob".to_vec(),
            ..make_presented("SHA256:same")
        };
        let waiter = thread::spawn(move || verifier(&presented));
        let challenge = wait_pending(&service, "session-1");
        assert_eq!(challenge.kind, HostIdentityChallengeKind::Changed);
        assert_eq!(challenge.stored_algorithm.as_deref(), Some("ssh-rsa"));
        service.reject(&challenge.challenge_id).unwrap();
        assert_eq!(
            waiter.join().unwrap().unwrap_err().code(),
            "HostKeyRejected"
        );
    }

    /// 仅本次接受以 Runtime Session 为作用域：Changed challenge 的临时接受只放行当前
    /// Session（含重连），其他 Session 相同 endpoint + 相同呈现 key 仍独立等待。
    #[test]
    fn changed_challenge_accept_once_is_scoped_to_current_session() {
        let app = mock_app();
        let (service, _path) = service_with_record(TrustRecord {
            host: "10.0.0.8".to_string(),
            port: 22,
            algorithm: "ssh-ed25519".to_string(),
            blob: b"stored".to_vec(),
        });
        let presented = PresentedHostKey {
            blob: b"rotated".to_vec(),
            ..make_presented("SHA256:rotated")
        };
        let verifier_a = service.verifier(app.handle().clone(), "session-a".to_string());
        let verifier_b = service.verifier(app.handle().clone(), "session-b".to_string());
        let waiter_a = thread::spawn({
            let verifier = verifier_a.clone();
            let presented = presented.clone();
            move || verifier(&presented)
        });
        let waiter_b = thread::spawn({
            let verifier = verifier_b.clone();
            let presented = presented.clone();
            move || verifier(&presented)
        });
        let challenge_a = wait_pending(&service, "session-a");
        let challenge_b = wait_pending(&service, "session-b");
        assert_eq!(challenge_a.kind, HostIdentityChallengeKind::Changed);
        assert_eq!(challenge_b.kind, HostIdentityChallengeKind::Changed);
        assert_ne!(challenge_a.challenge_id, challenge_b.challenge_id);

        // 仅本次接受只解决 session-a 的 challenge
        service.accept(&challenge_a.challenge_id).unwrap();
        waiter_a.join().unwrap().expect("session-a 放行");
        assert_eq!(
            service.pending_challenge("session-b").unwrap().challenge_id,
            challenge_b.challenge_id,
            "其他 Session 仍独立等待"
        );
        // 同 Session 重连复用临时信任，不产生新 challenge
        verifier_a(&presented).expect("session-a 重连放行");
        // session-b 按自己的决定解决
        service.accept(&challenge_b.challenge_id).unwrap();
        waiter_b.join().unwrap().expect("session-b 按自身决定放行");
    }

    /// 替换成功只自动解决兼容的 pending challenge：相同 endpoint + 相同呈现 key 的
    /// 其他 Session 一并放行；不同呈现 key 或不同 endpoint 不受影响。
    #[test]
    fn replace_releases_only_compatible_changed_challenges() {
        let app = mock_app();
        let (service, path) = service_with_record(TrustRecord {
            host: "10.0.0.8".to_string(),
            port: 22,
            algorithm: "ssh-ed25519".to_string(),
            blob: b"stored".to_vec(),
        });
        let presented = PresentedHostKey {
            blob: b"rotated".to_vec(),
            ..make_presented("SHA256:rotated")
        };
        let presented_other_key = PresentedHostKey {
            blob: b"other-key".to_vec(),
            ..make_presented("SHA256:other")
        };
        let presented_other_endpoint = PresentedHostKey {
            host: "10.0.0.9".to_string(),
            ..presented.clone()
        };
        let verifier_a = service.verifier(app.handle().clone(), "session-a".to_string());
        let verifier_b = service.verifier(app.handle().clone(), "session-b".to_string());
        let verifier_c = service.verifier(app.handle().clone(), "session-c".to_string());
        let verifier_d = service.verifier(app.handle().clone(), "session-d".to_string());
        let waiter_a = thread::spawn({
            let verifier = verifier_a.clone();
            let presented = presented.clone();
            move || verifier(&presented)
        });
        let waiter_b = thread::spawn({
            let verifier = verifier_b.clone();
            let presented = presented.clone();
            move || verifier(&presented)
        });
        let waiter_c = thread::spawn({
            let verifier = verifier_c.clone();
            let presented = presented_other_key.clone();
            move || verifier(&presented)
        });
        let waiter_d = thread::spawn({
            let verifier = verifier_d.clone();
            let presented = presented_other_endpoint.clone();
            move || verifier(&presented)
        });
        let challenge_a = wait_pending(&service, "session-a");
        let challenge_b = wait_pending(&service, "session-b");
        assert_eq!(challenge_b.kind, HostIdentityChallengeKind::Changed);
        let challenge_c = wait_pending(&service, "session-c");
        let challenge_d = wait_pending(&service, "session-d");

        // session-a 替换：同 endpoint + 同呈现 key 的 session-b 一并放行
        service
            .accept_and_save(app.handle(), &challenge_a.challenge_id)
            .unwrap();
        waiter_a.join().unwrap().expect("发起替换的 Session 放行");
        waiter_b.join().unwrap().expect("兼容 challenge 一并放行");
        // 不同呈现 key / 不同 endpoint 的 challenge 不受影响，仍待各自决定
        assert_eq!(
            service.pending_challenge("session-c").unwrap().challenge_id,
            challenge_c.challenge_id
        );
        assert_eq!(
            service.pending_challenge("session-d").unwrap().challenge_id,
            challenge_d.challenge_id
        );
        // 磁盘只保留呈现 key，且不新增其他 endpoint 记录
        let records = TrustStore::from_file_path(path).reload().unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].host, "10.0.0.8");
        assert_eq!(records[0].blob, b"rotated");
        // 清理剩余挑战
        service.reject(&challenge_c.challenge_id).unwrap();
        service.reject(&challenge_d.challenge_id).unwrap();
        waiter_c.join().unwrap().unwrap_err();
        waiter_d.join().unwrap().unwrap_err();
    }

    /// 替换写入失败：旧信任记录与 pending challenge 均保留，用户可明确改选
    /// 仅本次接受或拒绝；绝不静默降级或丢失旧记录。
    #[test]
    fn replace_write_failure_keeps_old_record_and_pending_challenge() {
        use std::os::unix::fs::PermissionsExt;

        let app = mock_app();
        let (service, path) = service_with_record(TrustRecord {
            host: "10.0.0.8".to_string(),
            port: 22,
            algorithm: "ssh-ed25519".to_string(),
            blob: b"old-blob".to_vec(),
        });
        let verifier = service.verifier(app.handle().clone(), "session-1".to_string());
        let presented = PresentedHostKey {
            blob: b"new-blob".to_vec(),
            ..make_presented("SHA256:new")
        };
        let waiter = thread::spawn({
            let verifier = verifier.clone();
            let presented = presented.clone();
            move || verifier(&presented)
        });
        let challenge = wait_pending(&service, "session-1");
        assert_eq!(challenge.kind, HostIdentityChallengeKind::Changed);

        // 禁止父目录创建临时发布文件，但保留 known_hosts 可读取，旧 key 的后续
        // verifier 才能验证失败替换没有污染持久化记录。
        let parent = path.parent().expect("信任文件应有父目录");
        let original_permissions = fs::metadata(parent).unwrap().permissions();
        let mut read_only_permissions = original_permissions.clone();
        read_only_permissions.set_mode(0o555);
        fs::set_permissions(parent, read_only_permissions).unwrap();
        let error = service
            .accept_and_save(app.handle(), &challenge.challenge_id)
            .unwrap_err();
        fs::set_permissions(parent, original_permissions).unwrap();
        assert_eq!(error.code(), "HostKeySaveFailed");
        // 旧信任记录未被失败替换污染：新 Session 呈现旧 key 仍精确匹配静默放行
        let verifier_old = service.verifier(app.handle().clone(), "session-old".to_string());
        verifier_old(&PresentedHostKey {
            blob: b"old-blob".to_vec(),
            ..make_presented("SHA256:old")
        })
        .expect("失败替换不得污染旧信任记录");
        // challenge 保持未决
        assert_eq!(
            service.pending_challenge("session-1").unwrap().challenge_id,
            challenge.challenge_id
        );
        // 用户明确改选仅本次接受：正常解决
        service.accept(&challenge.challenge_id).unwrap();
        waiter.join().unwrap().expect("改选仅本次接受后放行");
    }

    /// challenge 之后服务端再次更换 key：同一 Session、同一 endpoint 的新呈现 key
    /// 取代旧 challenge；旧等待者取消（不得以旧 key 认证），对旧 challenge 的
    /// 接受/替换/拒绝决定一律安全失败，新 challenge 正常可决。
    #[test]
    fn server_rotating_key_after_challenge_supersedes_old_challenge() {
        let app = mock_app();
        let events = Arc::new(AtomicUsize::new(0));
        let counter = events.clone();
        app.listen("host-identity:challenge", move |_| {
            counter.fetch_add(1, Ordering::Relaxed);
        });
        let (service, _path) = service_with_record(TrustRecord {
            host: "10.0.0.8".to_string(),
            port: 22,
            algorithm: "ssh-ed25519".to_string(),
            blob: b"stored".to_vec(),
        });
        let verifier = service.verifier(app.handle().clone(), "session-1".to_string());
        let presented_a = PresentedHostKey {
            blob: b"key-a".to_vec(),
            ..make_presented("SHA256:key-a")
        };
        let waiter_a = thread::spawn({
            let verifier = verifier.clone();
            let presented = presented_a.clone();
            move || verifier(&presented)
        });
        let challenge_a = wait_pending(&service, "session-1");

        // 服务端再次更换 key：新连接呈现 key-b
        let presented_b = PresentedHostKey {
            blob: b"key-b".to_vec(),
            ..make_presented("SHA256:key-b")
        };
        let waiter_b = thread::spawn(move || verifier(&presented_b));
        let challenge_b = wait_pending_other(&service, "session-1", &challenge_a.challenge_id);
        assert_ne!(challenge_a.challenge_id, challenge_b.challenge_id);
        assert_eq!(challenge_b.kind, HostIdentityChallengeKind::Changed);
        assert_eq!(challenge_b.fingerprint, "SHA256:key-b");

        // 旧等待者取消：连接不得以未经确认的旧 key 认证
        assert_eq!(
            waiter_a.join().unwrap().unwrap_err().code(),
            "HostKeyVerificationCancelled"
        );
        // 对旧 challenge 的一切决定安全失败（stale / 重复解决 / 未知 challengeId）
        assert_eq!(
            service
                .accept(&challenge_a.challenge_id)
                .unwrap_err()
                .code(),
            "HostKeyChallengeNotFound"
        );
        assert_eq!(
            service
                .accept_and_save(app.handle(), &challenge_a.challenge_id)
                .unwrap_err()
                .code(),
            "HostKeyChallengeNotFound"
        );
        assert_eq!(
            service
                .reject(&challenge_a.challenge_id)
                .unwrap_err()
                .code(),
            "HostKeyChallengeNotFound"
        );
        // 新 challenge 正常可决：替换为新呈现 key
        service
            .accept_and_save(app.handle(), &challenge_b.challenge_id)
            .unwrap();
        waiter_b.join().unwrap().expect("新 challenge 决定后放行");
        assert_eq!(
            events.load(Ordering::Relaxed),
            2,
            "两次不同 key 各派发一次 challenge 事件"
        );
    }

    /// 一个 Session 的决定不影响其他 Session 的 pending challenge：接受 session-a 的
    /// challenge 后 session-b 仍独立等待（错误 Session 不得解决他人 challenge）。
    #[test]
    fn decision_does_not_resolve_other_sessions_challenge() {
        let app = mock_app();
        let (service, _path) = service_with_record(TrustRecord {
            host: "10.0.0.8".to_string(),
            port: 22,
            algorithm: "ssh-ed25519".to_string(),
            blob: b"stored".to_vec(),
        });
        let verifier_a = service.verifier(app.handle().clone(), "session-a".to_string());
        let verifier_b = service.verifier(app.handle().clone(), "session-b".to_string());
        let presented = PresentedHostKey {
            blob: b"rotated".to_vec(),
            ..make_presented("SHA256:rotated")
        };
        let waiter_a = thread::spawn({
            let verifier = verifier_a.clone();
            let presented = presented.clone();
            move || verifier(&presented)
        });
        let waiter_b = thread::spawn({
            let verifier = verifier_b.clone();
            let presented = presented.clone();
            move || verifier(&presented)
        });
        let challenge_a = wait_pending(&service, "session-a");
        let challenge_b = wait_pending(&service, "session-b");

        service.accept(&challenge_a.challenge_id).unwrap();
        waiter_a.join().unwrap().unwrap();
        assert_eq!(
            service.pending_challenge("session-b").unwrap().challenge_id,
            challenge_b.challenge_id,
            "接受 session-a 不得解决 session-b 的 challenge"
        );
        // 重复/错误 Session 决定安全失败（stale challengeId），不得影响 session-b
        assert_eq!(
            service
                .accept(&challenge_a.challenge_id)
                .unwrap_err()
                .code(),
            "HostKeyChallengeNotFound"
        );
        assert_eq!(
            service
                .reject(&challenge_a.challenge_id)
                .unwrap_err()
                .code(),
            "HostKeyChallengeNotFound"
        );
        assert_eq!(
            service.pending_challenge("session-b").unwrap().challenge_id,
            challenge_b.challenge_id,
            "错误 Session 的决定不得取消或解决他人 challenge"
        );
        service.reject(&challenge_b.challenge_id).unwrap();
        assert_eq!(
            waiter_b.join().unwrap().unwrap_err().code(),
            "HostKeyRejected"
        );
    }

    /// 并发压力：保存/替换与「服务端再次换 key」（取代旧 challenge）竞争。
    /// 无论竞争顺序如何，stale 保存都不得授予旧 challenge 的临时信任；写盘在
    /// state 锁外，用户已选择保存时磁盘可保留旧 key 或保存 key-a，新 challenge
    /// 始终可解决。
    #[test]
    fn save_racing_supersede_never_persists_stale_key() {
        let app = mock_app();
        for _ in 0..25 {
            let (service, path) = service_with_record(TrustRecord {
                host: "10.0.0.8".to_string(),
                port: 22,
                algorithm: "ssh-ed25519".to_string(),
                blob: b"old".to_vec(),
            });
            let verifier = service.verifier(app.handle().clone(), "session-1".to_string());
            let presented_a = PresentedHostKey {
                blob: b"key-a".to_vec(),
                ..make_presented("SHA256:key-a")
            };
            let waiter_a = thread::spawn({
                let verifier = verifier.clone();
                let presented = presented_a.clone();
                move || verifier(&presented)
            });
            let challenge_a = wait_pending(&service, "session-1");

            // 并发：保存 challenge_a（替换记录）与呈现新 key（取代旧 challenge）
            let save_handle = {
                let service = service.clone();
                let app_handle = app.handle().clone();
                let challenge_id = challenge_a.challenge_id.clone();
                thread::spawn(move || service.accept_and_save(&app_handle, &challenge_id))
            };
            let presented_b = PresentedHostKey {
                blob: b"key-b".to_vec(),
                ..make_presented("SHA256:key-b")
            };
            let waiter_b = thread::spawn({
                let verifier = verifier.clone();
                move || verifier(&presented_b)
            });

            match save_handle.join().unwrap() {
                Ok(()) => {
                    // 保存成功：磁盘只保留呈现 key，challenge_a 等待者被接受
                    let records = TrustStore::from_file_path(path.clone()).reload().unwrap();
                    assert_eq!(records.len(), 1);
                    assert_eq!(records[0].blob, b"key-a");
                    waiter_a
                        .join()
                        .unwrap()
                        .expect("保存成功应放行本挑战等待者");
                }
                Err(error) => {
                    // stale 保存：不得放行 challenge_a；保存与取代竞争时，磁盘可能
                    // 仍是旧记录（取代先发生）或是用户已确认的 key-a（写盘先完成）。
                    assert_eq!(error.code(), "HostKeyChallengeNotFound");
                    let records = TrustStore::from_file_path(path.clone()).reload().unwrap();
                    assert!(
                        matches!(records[0].blob.as_slice(), b"old" | b"key-a"),
                        "stale 保存的磁盘结果只能是旧记录或用户确认的 key-a"
                    );
                    assert_eq!(
                        waiter_a.join().unwrap().unwrap_err().code(),
                        "HostKeyVerificationCancelled"
                    );
                }
            }
            // 新 challenge（key-b）始终正常可决：接受后放行，无死锁
            let challenge_b = wait_pending_other(&service, "session-1", &challenge_a.challenge_id);
            service.accept(&challenge_b.challenge_id).unwrap();
            waiter_b.join().unwrap().expect("新 challenge 接受后放行");
        }
    }
}
