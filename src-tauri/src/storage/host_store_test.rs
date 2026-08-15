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

    /// 旧路径不可复制时返回存储错误，不静默丢失主机配置
    #[test]
    fn migrate_legacy_hosts_reports_copy_failure() {
        let root = std::env::temp_dir().join(format!("titan-host-migration-{}", Uuid::new_v4()));
        let legacy_file = root.join("legacy-directory");
        let new_file = root.join("production/hosts.json");
        fs::create_dir_all(&legacy_file).expect("legacy directory should be created");
        fs::create_dir_all(new_file.parent().expect("production parent should exist"))
            .expect("production dir should be created");

        let error = migrate_legacy_hosts(&legacy_file, &new_file)
            .expect_err("copying a directory as hosts.json should fail");

        assert!(error.to_string().contains("迁移旧主机配置失败"));
    }

    #[test]
    fn save_and_load_round_trip_hosts() {
        let store = HostStore::from_file_path(temp_hosts_file());
        let hosts = vec![sample_host()];

        store.save(&hosts).expect("save should succeed");
        let loaded = store.load().expect("load should succeed");

        assert_eq!(loaded, hosts);
    }

    #[test]
    fn load_returns_error_for_invalid_json() {
        let file_path = temp_hosts_file();
        fs::write(&file_path, "{not-json").expect("invalid json should be written");
        let store = HostStore::from_file_path(file_path);

        let error = store.load().expect_err("load should fail");
        assert!(error.to_string().contains("解析主机配置文件失败"));
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
