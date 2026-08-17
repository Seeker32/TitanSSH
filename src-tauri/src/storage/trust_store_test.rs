#[cfg(test)]
mod tests {
    use crate::storage::trust_store::*;
    use std::fs;
    use std::thread;
    use uuid::Uuid;

    /// 在系统临时目录创建隔离的测试文件路径。
    fn temp_store_path() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("titan-trust-store-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).expect("temp dir should be created");
        dir.join(KNOWN_HOSTS_FILE_NAME)
    }

    fn store_at(path: &Path) -> TrustStore {
        TrustStore::from_file_path(path.to_path_buf())
    }

    fn record(host: &str, port: u16, algorithm: &str, blob: &[u8]) -> TrustRecord {
        TrustRecord {
            host: host.to_string(),
            port,
            algorithm: algorithm.to_string(),
            blob: blob.to_vec(),
        }
    }

    /// 缺失文件视为空信任存储，不产生错误。
    #[test]
    fn missing_file_is_empty_trust_store() {
        let path = temp_store_path();
        let store = store_at(&path);
        assert_eq!(store.lookup("10.0.0.8", 22).unwrap(), None);
    }

    /// 保存后按精确 endpoint 匹配读取，公钥 blob 字节级往返一致。
    #[test]
    fn save_and_lookup_round_trip_exact_key_material() {
        let path = temp_store_path();
        let store = store_at(&path);
        let blob = b"openssh-wire-blob-bytes".to_vec();
        store
            .upsert(record("10.0.0.8", 22, "ssh-ed25519", &blob))
            .unwrap();

        let found = store.lookup("10.0.0.8", 22).unwrap().unwrap();
        assert_eq!(found.host, "10.0.0.8");
        assert_eq!(found.port, 22);
        assert_eq!(found.algorithm, "ssh-ed25519");
        assert_eq!(found.blob, blob);
        assert!(found.matches("10.0.0.8", 22, "ssh-ed25519", &blob));
    }

    /// 已加载缓存必须发现外部删除和恢复，避免同一实例持续使用过期信任决策。
    #[test]
    fn lookup_reloads_after_external_file_removal_and_restore() {
        let path = temp_store_path();
        let store = store_at(&path);
        store
            .upsert(record("removed.example.com", 22, "ssh-ed25519", b"old-key"))
            .expect("预置记录应成功");
        assert!(
            store
                .lookup("removed.example.com", 22)
                .expect("首次查询应加载缓存")
                .is_some()
        );

        fs::remove_file(&path).expect("模拟外部删除应成功");
        assert_eq!(
            store
                .lookup("removed.example.com", 22)
                .expect("外部删除后查询应成功"),
            None,
            "外部删除后的记录不得继续被信任"
        );

        fs::write(&path, "restored.example.com ssh-ed25519 bmV3LWtleQ\n")
            .expect("模拟外部恢复应成功");
        let restored = store
            .lookup("restored.example.com", 22)
            .expect("外部恢复后查询应成功")
            .expect("外部恢复的记录应被加载");
        assert_eq!(restored.blob, b"new-key");
    }

    /// 清单读取同样必须发现外部替换，不能只在 lookup 时刷新缓存。
    #[test]
    fn list_reloads_after_external_file_replacement() {
        let path = temp_store_path();
        let store = store_at(&path);
        store
            .upsert(record("old.example.com", 22, "ssh-ed25519", b"old-key"))
            .expect("预置记录应成功");
        assert_eq!(store.list().expect("首次清单应加载缓存").len(), 1);

        fs::write(&path, "restored-new.example.com ssh-ed25519 bmV3LWtleQ\n")
            .expect("模拟外部替换应成功");
        let records = store.list().expect("外部替换后清单应成功");

        assert_eq!(records.len(), 1);
        assert_eq!(records[0].host, "restored-new.example.com");
        assert_eq!(records[0].blob, b"new-key");
    }

    /// 端口 22 的主机不带括号；磁盘内容为标准 OpenSSH 表示。
    #[test]
    fn default_port_uses_plain_host_pattern() {
        let path = temp_store_path();
        let store = store_at(&path);
        store
            .upsert(record("prod.example.com", 22, "ssh-rsa", b"blob"))
            .unwrap();
        let content = fs::read_to_string(&path).unwrap();
        assert_eq!(content, "prod.example.com ssh-rsa YmxvYg\n");
    }

    /// 非 22 端口使用 `[host]:port` 标准表示，重新加载后端口精确还原。
    #[test]
    fn non_default_port_uses_bracket_notation() {
        let path = temp_store_path();
        let store = store_at(&path);
        store
            .upsert(record("10.0.0.8", 2222, "ssh-ed25519", b"blob"))
            .unwrap();
        let content = fs::read_to_string(&path).unwrap();
        assert_eq!(content, "[10.0.0.8]:2222 ssh-ed25519 YmxvYg\n");
        let found = store.lookup("10.0.0.8", 2222).unwrap().unwrap();
        assert_eq!(found.port, 2222);
        // 同一主机不同端口互不干扰
        assert_eq!(store.lookup("10.0.0.8", 22).unwrap(), None);
    }

    /// IPv6 地址（含端口 22）使用 `[addr]:port` 标准表示并精确还原。
    #[test]
    fn ipv6_uses_bracket_notation_even_on_default_port() {
        let path = temp_store_path();
        let store = store_at(&path);
        store
            .upsert(record("2001:db8::1", 22, "ssh-ed25519", b"blob"))
            .unwrap();
        let content = fs::read_to_string(&path).unwrap();
        assert_eq!(content, "[2001:db8::1]:22 ssh-ed25519 YmxvYg\n");
        let found = store.lookup("2001:db8::1", 22).unwrap().unwrap();
        assert_eq!(found.host, "2001:db8::1");
    }

    /// endpoint 拼写精确保留：不做小写、尾点归一化，不同拼写是不同 endpoint。
    #[test]
    fn endpoint_spelling_is_exact() {
        let path = temp_store_path();
        let store = store_at(&path);
        store
            .upsert(record("Prod.Example.COM", 22, "ssh-ed25519", b"blob"))
            .unwrap();
        store
            .upsert(record("prod.example.com", 22, "ssh-ed25519", b"other"))
            .unwrap();
        store
            .upsert(record("example.com.", 22, "ssh-ed25519", b"trailing"))
            .unwrap();
        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("Prod.Example.COM ssh-ed25519 YmxvYg\n"));
        assert!(content.contains("prod.example.com ssh-ed25519 b3RoZXI\n"));
        assert!(content.contains("example.com. ssh-ed25519 dHJhaWxpbmc\n"));
    }

    /// 同一 endpoint 再次 upsert 只保留最新记录（一个当前算法 + 完整公钥）。
    #[test]
    fn upsert_keeps_single_record_per_endpoint() {
        let path = temp_store_path();
        let store = store_at(&path);
        store
            .upsert(record("10.0.0.8", 22, "ssh-rsa", b"old"))
            .unwrap();
        store
            .upsert(record("10.0.0.8", 22, "ssh-ed25519", b"new"))
            .unwrap();
        let found = store.lookup("10.0.0.8", 22).unwrap().unwrap();
        assert_eq!(found.algorithm, "ssh-ed25519");
        assert_eq!(found.blob, b"new");
        assert_eq!(
            fs::read_to_string(&path).unwrap().lines().count(),
            1,
            "同一 endpoint 只保留一条记录"
        );
    }

    /// 不可序列化的记录必须在写入前拒绝，不能污染已有的可解析信任文件。
    #[test]
    fn upsert_rejects_unserializable_records_without_poisoning_store() {
        let path = temp_store_path();
        let store = store_at(&path);
        store
            .upsert(record("trusted.example.com", 22, "ssh-ed25519", b"trusted"))
            .expect("预置合法记录应成功");
        let original_content = fs::read_to_string(&path).expect("应能读取预置记录");

        for (host, algorithm, blob) in [
            ("host with whitespace", "ssh-ed25519", b"blob" as &[u8]),
            ("#comment", "ssh-ed25519", b"blob"),
            ("host[", "ssh-ed25519", b"blob"),
            ("host]", "ssh-ed25519", b"blob"),
            ("*.example.com", "ssh-ed25519", b"blob"),
            ("host?.example.com", "ssh-ed25519", b"blob"),
            ("!blocked.example.com", "ssh-ed25519", b"blob"),
            ("a.example.com,b.example.com", "ssh-ed25519", b"blob"),
            ("@revoked.example.com", "ssh-ed25519", b"blob"),
            ("server.example.com", "ssh-ed25519 cert", b"blob"),
            ("server.example.com", "ssh-ed25519", b""),
        ] {
            let error = store
                .upsert(record(host, 22, algorithm, blob))
                .expect_err("不可序列化的记录必须被拒绝");

            assert_eq!(error.code(), "TrustStoreError");
            assert_eq!(
                fs::read_to_string(&path).expect("拒绝写入后原文件应可读取"),
                original_content,
                "拒绝写入不得修改已有信任文件"
            );
            assert_eq!(
                store
                    .lookup("trusted.example.com", 22)
                    .expect("拒绝写入不得破坏已有缓存")
                    .expect("预置记录应保持存在")
                    .blob,
                b"trusted"
            );
        }
    }

    /// 不可解析文件 fail-closed：load 返回 TrustStoreError，绝不静默视为空。
    #[test]
    fn corrupt_file_fails_closed() {
        let path = temp_store_path();
        fs::write(&path, "10.0.0.8 ssh-ed25519\n").unwrap();
        let store = store_at(&path);
        let error = store.lookup("10.0.0.8", 22).unwrap_err();
        assert_eq!(error.code(), "TrustStoreError");

        // 非法 base64 同样 fail-closed
        let path2 = temp_store_path();
        fs::write(&path2, "10.0.0.8 ssh-ed25519 not-base64!!!\n").unwrap();
        let store2 = store_at(&path2);
        assert_eq!(
            store2.lookup("10.0.0.8", 22).unwrap_err().code(),
            "TrustStoreError"
        );

        // 无括号含冒号的模糊主机段也 fail-closed
        let path3 = temp_store_path();
        fs::write(&path3, "::1 ssh-ed25519 blob\n").unwrap();
        let store3 = store_at(&path3);
        assert_eq!(
            store3.lookup("::1", 22).unwrap_err().code(),
            "TrustStoreError"
        );
    }

    /// 外部文件中同一 endpoint 出现多个记录时 fail-closed，不能按文件顺序任选一个密钥。
    #[test]
    fn duplicate_endpoint_records_fail_closed() {
        let path = temp_store_path();
        fs::write(
            &path,
            "duplicate.example.com ssh-ed25519 b2xk\nduplicate.example.com ssh-ed25519 bmV3\n",
        )
        .expect("写入外部重复记录应成功");
        let store = store_at(&path);

        assert_eq!(
            store
                .lookup("duplicate.example.com", 22)
                .unwrap_err()
                .code(),
            "TrustStoreError"
        );
        assert_eq!(store.list().unwrap_err().code(), "TrustStoreError");
    }

    /// 不可读文件（路径为目录）fail-closed。
    #[test]
    fn unreadable_file_fails_closed() {
        let dir = std::env::temp_dir().join(format!("titan-trust-dir-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).expect("temp dir should be created");
        let store = TrustStore::from_file_path(dir.clone());
        let error = store.lookup("10.0.0.8", 22).unwrap_err();
        assert_eq!(error.code(), "TrustStoreError");
    }

    /// 外部将信任文件替换为目录后，写入与后续读取均 fail-closed，不能继续使用旧缓存。
    #[test]
    fn external_unreadable_replacement_fails_closed_after_upsert_error() {
        let path = temp_store_path();
        let store = store_at(&path);
        store
            .upsert(record("10.0.0.8", 22, "ssh-ed25519", b"old"))
            .unwrap();
        // 缓存已加载后破坏文件路径：目标替换为目录，发布必然失败
        fs::remove_file(&path).unwrap();
        fs::create_dir_all(&path).unwrap();
        let error = store
            .upsert(record("10.0.0.8", 22, "ssh-ed25519", b"new"))
            .unwrap_err();
        assert_eq!(error.code(), "TrustStoreError");
        assert_eq!(
            store.lookup("10.0.0.8", 22).unwrap_err().code(),
            "TrustStoreError",
            "外部替换后不得继续使用旧缓存"
        );
    }

    /// 空行与注释行跳过，其余行正常解析。
    #[test]
    fn blank_lines_and_comments_are_skipped() {
        let path = temp_store_path();
        fs::write(
            &path,
            "# TitanSSH trust store\n\n10.0.0.8 ssh-ed25519 blob\n\n",
        )
        .unwrap();
        let store = store_at(&path);
        let records = store.reload().unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].host, "10.0.0.8");
        assert_eq!(records[0].port, 22);
    }

    /// 并发 upsert 不同 endpoint：串行化 + 读改写不丢失任何记录。
    #[test]
    fn concurrent_upserts_preserve_all_records() {
        let path = temp_store_path();
        let store = store_at(&path);
        let handles: Vec<_> = (0..8)
            .map(|index| {
                let store = store.clone();
                thread::spawn(move || {
                    store
                        .upsert(record(
                            &format!("10.0.0.{index}"),
                            22,
                            "ssh-ed25519",
                            format!("blob-{index}").as_bytes(),
                        ))
                        .unwrap();
                })
            })
            .collect();
        for handle in handles {
            handle.join().unwrap();
        }
        let records = store.reload().unwrap();
        assert_eq!(records.len(), 8, "并发保存不得丢失任何 endpoint 记录");
        for index in 0..8 {
            let found = store
                .lookup(&format!("10.0.0.{index}"), 22)
                .unwrap()
                .unwrap();
            assert_eq!(found.blob, format!("blob-{index}").as_bytes());
        }
    }

    /// 同 endpoint 并发 upsert：最终只保留一条记录（最后写入者胜出），文件不损坏。
    #[test]
    fn concurrent_upserts_same_endpoint_keep_single_record() {
        let path = temp_store_path();
        let store = store_at(&path);
        let handles: Vec<_> = (0..6)
            .map(|index| {
                let store = store.clone();
                thread::spawn(move || {
                    store
                        .upsert(record(
                            "10.0.0.8",
                            22,
                            "ssh-ed25519",
                            format!("blob-{index}").as_bytes(),
                        ))
                        .unwrap();
                })
            })
            .collect();
        for handle in handles {
            handle.join().unwrap();
        }
        let records = store.reload().unwrap();
        assert_eq!(records.len(), 1);
        assert!(
            records[0].blob.starts_with(b"blob-"),
            "最终记录必须是某一次完整写入"
        );
    }

    /// 生产构造路径：通过 AppHandle 解析应用数据目录并定位 known_hosts 文件。
    /// mock app 的应用数据目录在测试间共享：使用唯一 host 避免与其他测试的
    /// init_trust_store 写入互相干扰。
    #[test]
    fn new_resolves_app_data_dir_path() {
        let app = tauri::test::mock_app();
        let store = TrustStore::new(&app.handle()).expect("mock app 应可解析应用数据目录");
        let unique_host = format!(
            "10.0.0.{}",
            uuid::Uuid::new_v4().simple().to_string()[..8].to_string()
        );
        store
            .upsert(record(&unique_host, 22, "ssh-ed25519", b"blob"))
            .expect("生产路径写入应成功");
        let found = store.lookup(&unique_host, 22).unwrap().unwrap();
        assert_eq!(found.blob, b"blob");
    }

    /// 移除精确 endpoint 的记录并安全发布：磁盘不再包含该记录，其他 endpoint 不受影响。
    #[test]
    fn remove_deletes_record_and_persists() {
        let path = temp_store_path();
        let store = store_at(&path);
        store
            .upsert(record("10.0.0.8", 22, "ssh-ed25519", b"blob-a"))
            .unwrap();
        store
            .upsert(record("10.0.0.9", 22, "ssh-ed25519", b"blob-b"))
            .unwrap();

        store.remove("10.0.0.8", 22).unwrap();
        assert_eq!(store.lookup("10.0.0.8", 22).unwrap(), None);
        assert_eq!(
            store.lookup("10.0.0.9", 22).unwrap().unwrap().blob,
            b"blob-b",
            "移除不得影响其他 endpoint 的记录"
        );
        // 磁盘真实内容同步更新（绕过内存缓存重读）
        let records = store.reload().unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].host, "10.0.0.9");
    }

    /// 移除不存在的 endpoint 幂等成功：不报错，也不产生磁盘写入。
    #[test]
    fn remove_missing_endpoint_is_idempotent() {
        let path = temp_store_path();
        let store = store_at(&path);
        store
            .upsert(record("10.0.0.8", 22, "ssh-ed25519", b"blob-a"))
            .unwrap();

        store.remove("10.0.0.7", 22).unwrap();
        assert_eq!(
            store.lookup("10.0.0.8", 22).unwrap().unwrap().blob,
            b"blob-a"
        );
        assert_eq!(store.reload().unwrap().len(), 1);
    }

    /// 外部将信任文件替换为目录后，移除与后续读取均 fail-closed，不能继续使用旧缓存。
    #[test]
    fn external_unreadable_replacement_fails_closed_after_remove_error() {
        let path = temp_store_path();
        let store = store_at(&path);
        store
            .upsert(record("10.0.0.8", 22, "ssh-ed25519", b"blob-a"))
            .unwrap();
        // 缓存已加载后破坏发布目标：路径替换为目录，发布必然失败
        fs::remove_file(&path).unwrap();
        fs::create_dir_all(&path).unwrap();

        let error = store.remove("10.0.0.8", 22).unwrap_err();
        assert_eq!(error.code(), "TrustStoreError");
        assert_eq!(
            store.lookup("10.0.0.8", 22).unwrap_err().code(),
            "TrustStoreError",
            "外部替换后不得继续使用旧缓存"
        );
    }

    /// list 返回全部记录并按 host 字典序 + port 稳定排序（Settings 清单的稳定展示顺序）。
    #[test]
    fn list_returns_stable_sorted_order() {
        let path = temp_store_path();
        let store = store_at(&path);
        for (host, port) in [
            ("b.example.com", 22),
            ("a.example.com", 2222),
            ("a.example.com", 22),
            ("c.example.com", 22),
        ] {
            store
                .upsert(record(
                    host,
                    port,
                    "ssh-ed25519",
                    format!("blob-{host}-{port}").as_bytes(),
                ))
                .unwrap();
        }

        let records = store.list().unwrap();
        let order: Vec<(String, u16)> = records
            .iter()
            .map(|record| (record.host.clone(), record.port))
            .collect();
        assert_eq!(
            order,
            vec![
                ("a.example.com".to_string(), 22),
                ("a.example.com".to_string(), 2222),
                ("b.example.com".to_string(), 22),
                ("c.example.com".to_string(), 22),
            ],
            "清单按 host 字典序 + port 稳定排序"
        );
    }

    /// 空信任存储的 list 返回空列表；不可解析文件 list 同样 fail-closed。
    #[test]
    fn list_empty_store_and_corrupt_file() {
        let path = temp_store_path();
        let store = store_at(&path);
        assert_eq!(store.list().unwrap(), Vec::new());

        fs::write(&path, "10.0.0.8 ssh-ed25519\n").unwrap();
        let corrupt = TrustStore::from_file_path(path.clone());
        assert_eq!(corrupt.list().unwrap_err().code(), "TrustStoreError");
    }

    /// 主机段格式序列化遵循 OpenSSH 规则（单元级向量）。
    #[test]
    fn host_pattern_serialization_follows_openssh_rules() {
        assert_eq!(format_host_pattern("10.0.0.8", 22), "10.0.0.8");
        assert_eq!(format_host_pattern("10.0.0.8", 2222), "[10.0.0.8]:2222");
        assert_eq!(format_host_pattern("::1", 22), "[::1]:22");
        assert_eq!(
            format_host_pattern("2001:db8::1", 2200),
            "[2001:db8::1]:2200"
        );
        assert_eq!(
            parse_host_pattern("10.0.0.8").unwrap(),
            ("10.0.0.8".to_string(), 22)
        );
        assert_eq!(
            parse_host_pattern("[10.0.0.8]:2222").unwrap(),
            ("10.0.0.8".to_string(), 2222)
        );
        assert_eq!(
            parse_host_pattern("[::1]:22").unwrap(),
            ("::1".to_string(), 22)
        );
        assert!(parse_host_pattern("[10.0.0.8]").is_err());
        assert!(parse_host_pattern("[10.0.0.8]:notaport").is_err());
        assert!(parse_host_pattern("10.0.0.8:2222").is_err());
        assert!(parse_host_pattern("").is_err());
    }
}
