#[cfg(test)]
mod tests {
    use crate::models::host::{AuthType, HostConfig};
    use crate::storage::host_store::{HostStore, migrate_legacy_hosts};
    use proptest::prelude::*;
    use std::fs;
    use std::path::PathBuf;
    use uuid::Uuid;

    fn temp_hosts_file() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("titan-host-store-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).expect("temp dir should be created");
        dir.join("hosts.json")
    }

    fn sample_host() -> HostConfig {
        HostConfig {
            id: "host-1".to_string(),
            name: "prod".to_string(),
            host: "10.0.0.8".to_string(),
            port: 22,
            username: "root".to_string(),
            auth_type: AuthType::Password,
            password_ref: Some("titanssh-host-1-password".to_string()),
            private_key_path: None,
            passphrase_ref: None,
            remark: Some("primary".to_string()),
            group: "production".to_string(),
        }
    }

    #[test]
    fn load_returns_empty_when_file_does_not_exist() {
        let store = HostStore::from_file_path(temp_hosts_file());
        let hosts = store.load().expect("load should succeed");
        assert!(hosts.is_empty());
    }

    /// 正式目录尚无配置时复制开发期 hosts.json
    #[test]
    fn migrate_legacy_hosts_copies_missing_production_file() {
        let root = std::env::temp_dir().join(format!("titan-host-migration-{}", Uuid::new_v4()));
        let legacy_file = root.join("legacy/hosts.json");
        let new_file = root.join("production/hosts.json");
        fs::create_dir_all(legacy_file.parent().expect("legacy parent should exist"))
            .expect("legacy dir should be created");
        fs::create_dir_all(new_file.parent().expect("production parent should exist"))
            .expect("production dir should be created");
        fs::write(&legacy_file, "legacy hosts").expect("legacy hosts should be written");

        migrate_legacy_hosts(&legacy_file, &new_file).expect("migration should succeed");

        assert_eq!(
            fs::read_to_string(new_file).expect("production hosts should exist"),
            "legacy hosts"
        );
    }

    /// 正式目录已有配置时不得被开发期文件覆盖
    #[test]
    fn migrate_legacy_hosts_preserves_existing_production_file() {
        let root = std::env::temp_dir().join(format!("titan-host-migration-{}", Uuid::new_v4()));
        let legacy_file = root.join("legacy/hosts.json");
        let new_file = root.join("production/hosts.json");
        fs::create_dir_all(legacy_file.parent().expect("legacy parent should exist"))
            .expect("legacy dir should be created");
        fs::create_dir_all(new_file.parent().expect("production parent should exist"))
            .expect("production dir should be created");
        fs::write(&legacy_file, "legacy hosts").expect("legacy hosts should be written");
        fs::write(&new_file, "production hosts").expect("production hosts should be written");

        migrate_legacy_hosts(&legacy_file, &new_file).expect("migration should be a no-op");

        assert_eq!(
            fs::read_to_string(new_file).expect("production hosts should exist"),
            "production hosts"
        );
    }

    /// 旧路径不可读取时保留诊断错误，供 HostStore::new 记录日志后继续启动；
    /// 旧路径恢复正常后，后续启动可重试迁移。
    #[test]
    fn migrate_legacy_hosts_reports_read_failure_and_can_retry() {
        let root = std::env::temp_dir().join(format!("titan-host-migration-{}", Uuid::new_v4()));
        let legacy_file = root.join("legacy-directory");
        let new_file = root.join("production/hosts.json");
        fs::create_dir_all(&legacy_file).expect("legacy directory should be created");
        fs::create_dir_all(new_file.parent().expect("production parent should exist"))
            .expect("production dir should be created");

        let error = migrate_legacy_hosts(&legacy_file, &new_file)
            .expect_err("reading a directory as hosts.json should fail");

        assert!(error.to_string().contains("读取旧主机配置失败"));
        assert!(
            !new_file.exists(),
            "失败的可选迁移不得创建或破坏正式 hosts.json"
        );

        fs::remove_dir(&legacy_file).expect("legacy directory should be removable");
        fs::write(&legacy_file, "legacy hosts").expect("legacy hosts should be written");
        migrate_legacy_hosts(&legacy_file, &new_file).expect("repaired legacy source should retry");
        assert_eq!(
            fs::read_to_string(new_file).expect("production hosts should exist"),
            "legacy hosts"
        );
    }

    #[test]
    fn save_and_load_round_trip_hosts() {
        let store = HostStore::from_file_path(temp_hosts_file());
        let hosts = vec![sample_host()];

        store.save(&hosts).expect("save should succeed");
        let loaded = store.load().expect("load should succeed");

        assert_eq!(loaded, hosts);
    }

    /// 保存成功后目录内只有 hosts.json,无临时文件残留(原子写 tmp+rename)
    #[test]
    fn save_leaves_no_temp_file_residue() {
        let file_path = temp_hosts_file();
        let store = HostStore::from_file_path(file_path.clone());
        store.save(&[sample_host()]).expect("save should succeed");
        store
            .save(&[sample_host()])
            .expect("overwrite save should succeed");

        let names: Vec<String> = fs::read_dir(file_path.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["hosts.json".to_string()], "不得残留临时文件");
    }

    /// hosts.json 只读但目录可写:tmp+rename 原子写可成功替换。
    /// 修复前 fs::write 直写只读文件必然失败,且直写存在截断风险。
    #[cfg(unix)]
    #[test]
    fn save_replaces_readonly_file_via_atomic_rename() {
        use std::os::unix::fs::PermissionsExt;
        let file_path = temp_hosts_file();
        fs::write(&file_path, "[]").expect("seed hosts.json");
        fs::set_permissions(&file_path, fs::Permissions::from_mode(0o444)).unwrap();
        let store = HostStore::from_file_path(file_path.clone());
        let hosts = vec![sample_host()];

        store.save(&hosts).expect("rename 替换只读文件应成功");

        assert_eq!(store.load().expect("load should succeed"), hosts);
        let names: Vec<String> = fs::read_dir(file_path.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["hosts.json".to_string()], "不得残留临时文件");
    }

    /// 写入失败(目录只读)时原 hosts.json 完整保留,且无临时文件残留
    #[cfg(unix)]
    #[test]
    fn save_failure_preserves_original_file_and_leaves_no_temp() {
        use std::os::unix::fs::PermissionsExt;
        let file_path = temp_hosts_file();
        let original = "[]";
        fs::write(&file_path, original).expect("seed hosts.json");
        let dir = file_path.parent().unwrap().to_path_buf();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o555)).unwrap();
        let store = HostStore::from_file_path(file_path.clone());

        let result = store.save(&[sample_host()]);

        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(result.is_err(), "目录不可写时 save 必须失败");
        assert_eq!(
            fs::read_to_string(&file_path).unwrap(),
            original,
            "原内容必须完整保留,不得截断"
        );
        let names: Vec<String> = fs::read_dir(&dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["hosts.json".to_string()], "不得残留临时文件");
    }

    /// rename 失败(目标路径被目录占据)时返回错误并清理临时文件
    #[test]
    fn save_rename_failure_cleans_up_temp_file() {
        let file_path = temp_hosts_file();
        fs::create_dir(&file_path).expect("hosts.json 路径占位为目录");
        let store = HostStore::from_file_path(file_path.clone());

        let result = store.save(&[sample_host()]);

        assert!(result.is_err(), "rename 到目录必须失败");
        let names: Vec<String> = fs::read_dir(file_path.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["hosts.json".to_string()], "临时文件必须被清理");
    }

    #[test]
    fn load_returns_error_for_invalid_json() {
        let file_path = temp_hosts_file();
        fs::write(&file_path, "{not-json").expect("invalid json should be written");
        let store = HostStore::from_file_path(file_path);

        let error = store.load().expect_err("load should fail");
        assert!(error.to_string().contains("解析主机配置文件失败"));
    }

    /// 解析失败时必须隔离损坏配置，使下一次启动可将其视为空配置，且保留原始内容
    /// 供用户恢复。
    #[test]
    fn load_quarantines_corrupt_file_and_allows_subsequent_empty_load() {
        let file_path = temp_hosts_file();
        let corrupt_content = "{not-json";
        fs::write(&file_path, corrupt_content).expect("invalid json should be written");
        let store = HostStore::from_file_path(file_path.clone());

        let error = store
            .load()
            .expect_err("corrupt hosts file should report an error");
        assert!(error.to_string().contains("解析主机配置文件失败"));
        assert!(
            !file_path.exists(),
            "损坏文件必须从 hosts.json 原路径移走，避免阻断后续加载"
        );

        let backup_path = fs::read_dir(file_path.parent().expect("parent should exist"))
            .expect("host directory should be readable")
            .map(|entry| entry.expect("directory entry should be readable").path())
            .find(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("hosts.json.corrupt-"))
            })
            .expect("损坏文件必须被保留为独立备份");
        assert_eq!(
            fs::read_to_string(backup_path).expect("corrupt backup should be readable"),
            corrupt_content,
            "备份必须保留原始损坏内容"
        );
        assert!(
            store
                .load()
                .expect("quarantine 后的下一次加载应视为空配置")
                .is_empty()
        );
    }

    /// 生成任意合法 AuthType 的策略
    fn arb_auth_type() -> impl Strategy<Value = AuthType> {
        prop_oneof![Just(AuthType::Password), Just(AuthType::PrivateKey)]
    }

    /// 生成非空字符串的策略（至少1个可打印字符，最多64个字符）
    fn arb_nonempty_string() -> impl Strategy<Value = String> {
        "[a-zA-Z0-9_\\-\\.]{1,64}".prop_map(|s| s)
    }

    /// 生成任意合法 HostConfig 的策略
    /// - 非敏感字段使用合法字符串
    /// - 敏感字段仅使用引用键格式（titanssh:<id>:<field>），不含明文凭据
    fn arb_host_config() -> impl Strategy<Value = HostConfig> {
        (
            arb_nonempty_string(),                       // id
            arb_nonempty_string(),                       // name
            arb_nonempty_string(),                       // host
            1u16..=65535u16,                             // port
            arb_nonempty_string(),                       // username
            arb_auth_type(),                             // auth_type
            proptest::option::of(arb_nonempty_string()), // private_key_path
            proptest::option::of(arb_nonempty_string()), // remark
            arb_nonempty_string(),                       // group
        )
            .prop_map(
                |(id, name, host, port, username, auth_type, private_key_path, remark, group)| {
                    // 敏感字段仅以引用键形式存在，格式为 titanssh-<id>-<field>
                    let password_ref = if auth_type == AuthType::Password {
                        Some(format!("titanssh-{}-password", id))
                    } else {
                        None
                    };
                    let passphrase_ref = if auth_type == AuthType::PrivateKey {
                        Some(format!("titanssh-{}-passphrase", id))
                    } else {
                        None
                    };
                    HostConfig {
                        id,
                        name,
                        host,
                        port,
                        username,
                        auth_type,
                        password_ref,
                        private_key_path,
                        passphrase_ref,
                        remark,
                        group,
                    }
                },
            )
    }

    proptest! {
        /// 使用 proptest 生成任意合法 HostConfig，save 后 load 验证：
        /// 1. 非敏感字段（id, name, host, port, username, auth_type, private_key_path, remark）完全一致
        /// 2. 敏感字段（password_ref, passphrase_ref）仅以引用键形式存在，不含明文凭据
        #[test]
        fn prop_host_config_persistence_round_trip(host in arb_host_config()) {
            // 使用临时目录隔离文件 IO，避免测试间干扰
            let store = HostStore::from_file_path(temp_hosts_file());
            let hosts = vec![host.clone()];

            // 保存后重新加载
            store.save(&hosts).expect("save 应成功");
            let loaded = store.load().expect("load 应成功");

            prop_assert_eq!(loaded.len(), 1, "加载后应有且仅有一条记录");
            let loaded_host = &loaded[0];

            // 验证非敏感字段完全一致
            prop_assert_eq!(&loaded_host.id, &host.id, "id 应一致");
            prop_assert_eq!(&loaded_host.name, &host.name, "name 应一致");
            prop_assert_eq!(&loaded_host.host, &host.host, "host 应一致");
            prop_assert_eq!(loaded_host.port, host.port, "port 应一致");
            prop_assert_eq!(&loaded_host.username, &host.username, "username 应一致");
            prop_assert_eq!(&loaded_host.auth_type, &host.auth_type, "auth_type 应一致");
            prop_assert_eq!(&loaded_host.private_key_path, &host.private_key_path, "private_key_path 应一致");
            prop_assert_eq!(&loaded_host.remark, &host.remark, "remark 应一致");
            prop_assert_eq!(&loaded_host.group, &host.group, "group 应一致");

            // 验证敏感字段仅以引用形式存在（不含明文密码或口令）
            // password_ref 若存在，必须是引用键格式（以 "titanssh-" 开头），不得是明文密码
            if let Some(ref pw_ref) = loaded_host.password_ref {
                prop_assert!(
                    pw_ref.starts_with("titanssh-"),
                    "password_ref 必须是引用键格式，不得含明文密码，实际值: {}",
                    pw_ref
                );
            }
            // passphrase_ref 若存在，必须是引用键格式，不得是明文口令
            if let Some(ref pp_ref) = loaded_host.passphrase_ref {
                prop_assert!(
                    pp_ref.starts_with("titanssh-"),
                    "passphrase_ref 必须是引用键格式，不得含明文口令，实际值: {}",
                    pp_ref
                );
            }

            // 验证敏感字段引用与原始一致（引用键本身应被正确持久化）
            prop_assert_eq!(&loaded_host.password_ref, &host.password_ref, "password_ref 应一致");
            prop_assert_eq!(&loaded_host.passphrase_ref, &host.passphrase_ref, "passphrase_ref 应一致");
        }
    }
}
