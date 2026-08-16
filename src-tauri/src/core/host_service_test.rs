#[cfg(test)]
mod tests {
    use crate::core::host_identity::PresentedHostKey;
    use crate::core::host_service::*;
    use crate::errors::app_error::ErrorDetail;
    use crate::models::session::HostIdentityChallenge;
    use crate::storage::trust_store::{TrustRecord, TrustStore};
    use proptest::prelude::*;
    use std::collections::HashMap;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::{Duration, Instant};
    use uuid::Uuid;

    // --- 测试基础设施 ---

    /// 生成临时 hosts.json 路径(创建父目录)
    fn temp_hosts_file() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("titan-host-service-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).expect("temp dir should be created");
        dir.join("hosts.json")
    }

    /// 生成指向不可写目录的 hosts.json 路径:父目录不存在,fs::write 必然失败
    fn unwritable_hosts_file() -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("titan-host-service-missing-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).expect("temp dir should be created");
        dir.join("nested").join("hosts.json")
    }

    /// 内存凭据存储：记录写入/删除结果，可针对特定 key 注入失败
    #[derive(Default)]
    struct MemoryCredentialStore {
        entries: Mutex<HashMap<String, String>>,
        fail_set_key: Mutex<Option<String>>,
        fail_delete_key: Mutex<Option<String>>,
    }

    /// 内存信任清理：记录调用并支持注入失败
    #[derive(Default)]
    struct MemoryTrustCleanup {
        calls: Mutex<Vec<(String, u16)>>,
        fail: Mutex<bool>,
    }

    impl MemoryTrustCleanup {
        /// 快照全部清理调用（host, port），供断言
        fn calls(&self) -> Vec<(String, u16)> {
            self.calls.lock().unwrap().clone()
        }

        /// 注入下一次清理失败
        fn fail_next(&self) {
            *self.fail.lock().unwrap() = true;
        }
    }

    impl TrustRecordCleanup for Arc<MemoryTrustCleanup> {
        fn forget_endpoint(&self, host: &str, port: u16) -> Result<(), AppError> {
            if *self.fail.lock().unwrap() {
                return Err(AppError::TrustStoreError(ErrorDetail::msg(
                    "注入的清理失败",
                    Vec::new(),
                )));
            }
            self.calls.lock().unwrap().push((host.to_string(), port));
            Ok(())
        }
    }

    impl MemoryCredentialStore {
        fn new() -> Self {
            Self::default()
        }

        /// 快照当前条目,供断言
        fn entries(&self) -> HashMap<String, String> {
            self.entries.lock().unwrap().clone()
        }

        /// 注入对该 key 的写入失败
        fn fail_set_for(&self, key: &str) {
            *self.fail_set_key.lock().unwrap() = Some(key.to_string());
        }

        /// 注入对该 key 的删除失败
        fn fail_delete_for(&self, key: &str) {
            *self.fail_delete_key.lock().unwrap() = Some(key.to_string());
        }
    }

    impl CredentialStore for Arc<MemoryCredentialStore> {
        fn set(&self, key: &str, value: &str) -> Result<(), AppError> {
            let fail_key = self.fail_set_key.lock().unwrap();
            if fail_key.as_deref() == Some(key) {
                return Err(AppError::SecureStoreError(ErrorDetail::msg(
                    "注入的写入失败",
                    Vec::new(),
                )));
            }
            self.entries
                .lock()
                .unwrap()
                .insert(key.to_string(), value.to_string());
            Ok(())
        }

        fn delete(&self, key: &str) -> Result<(), AppError> {
            let fail_key = self.fail_delete_key.lock().unwrap();
            if fail_key.as_deref() == Some(key) {
                return Err(AppError::SecureStoreError(ErrorDetail::msg(
                    "注入的删除失败",
                    Vec::new(),
                )));
            }
            self.entries.lock().unwrap().remove(key);
            Ok(())
        }
    }

    /// 构造测试服务:内存凭据存储 + 临时文件 HostStore
    fn test_service() -> (Arc<MemoryCredentialStore>, HostConfigService) {
        let (credentials, service, _) = test_service_with_path();
        (credentials, service)
    }

    /// 构造测试服务，并返回 hosts.json 路径供原始内容断言
    fn test_service_with_path() -> (Arc<MemoryCredentialStore>, HostConfigService, PathBuf) {
        let (credentials, service, file_path, _) = test_service_with_path_and_cleanup();
        (credentials, service, file_path)
    }

    /// 构造带可观察信任清理的测试服务
    fn test_service_with_cleanup() -> (
        Arc<MemoryCredentialStore>,
        HostConfigService,
        Arc<MemoryTrustCleanup>,
    ) {
        let (credentials, service, _, cleanup) = test_service_with_path_and_cleanup();
        (credentials, service, cleanup)
    }

    /// 构造测试服务，并返回 hosts.json 路径与信任清理供断言
    fn test_service_with_path_and_cleanup() -> (
        Arc<MemoryCredentialStore>,
        HostConfigService,
        PathBuf,
        Arc<MemoryTrustCleanup>,
    ) {
        let credentials = Arc::new(MemoryCredentialStore::new());
        let cleanup = Arc::new(MemoryTrustCleanup::default());
        let file_path = temp_hosts_file();
        let store = HostStore::from_file_path(file_path.clone());
        let service = HostConfigService::with_stores(
            store,
            Box::new(credentials.clone()),
            Box::new(cleanup.clone()),
        );
        (credentials, service, file_path, cleanup)
    }

    /// 构造指向不可写目录的测试服务
    fn unwritable_service() -> (Arc<MemoryCredentialStore>, HostConfigService) {
        let (credentials, service, _) = unwritable_service_with_cleanup();
        (credentials, service)
    }

    /// 构造指向不可写目录的测试服务，并返回信任清理供断言
    fn unwritable_service_with_cleanup() -> (
        Arc<MemoryCredentialStore>,
        HostConfigService,
        Arc<MemoryTrustCleanup>,
    ) {
        let credentials = Arc::new(MemoryCredentialStore::new());
        let cleanup = Arc::new(MemoryTrustCleanup::default());
        let store = HostStore::from_file_path(unwritable_hosts_file());
        let service = HostConfigService::with_stores(
            store,
            Box::new(credentials.clone()),
            Box::new(cleanup.clone()),
        );
        (credentials, service, cleanup)
    }

    /// 生成基础保存请求(Password 认证,无凭据)
    fn sample_request(id: &str, name: &str) -> SaveHostRequest {
        SaveHostRequest {
            id: id.to_string(),
            name: name.to_string(),
            host: "10.0.0.1".to_string(),
            port: 22,
            username: "root".to_string(),
            auth_type: AuthType::Password,
            password: None,
            private_key_path: None,
            passphrase: None,
            remark: None,
            group: String::new(),
        }
    }

    /// 生成带明文密码的保存请求
    fn request_with_password(id: &str, password: &str) -> SaveHostRequest {
        SaveHostRequest {
            password: Some(password.to_string()),
            ..sample_request(id, "prod")
        }
    }

    // --- 校验(通过生产 composition 入口) ---

    #[test]
    fn save_rejects_blank_name() {
        let (_, service) = test_service();
        let req = sample_request("id1", "   ");
        assert!(service.save(&req).is_err());
        assert!(
            service.list_hosts().unwrap().is_empty(),
            "校验失败不得写入任何内容"
        );
    }

    #[test]
    fn save_rejects_blank_host() {
        let (_, service) = test_service();
        let req = SaveHostRequest {
            host: "\t".to_string(),
            ..sample_request("id2", "prod")
        };
        assert!(service.save(&req).is_err());
    }

    #[test]
    fn save_rejects_blank_username() {
        let (_, service) = test_service();
        let req = SaveHostRequest {
            username: String::new(),
            ..sample_request("id3", "prod")
        };
        assert!(service.save(&req).is_err());
    }

    #[test]
    fn save_accepts_valid_request() {
        let (_, service) = test_service();
        let result = service.save(&sample_request("id4", "prod"));
        assert!(result.is_ok());
    }

    // --- 保存:成功路径 ---

    #[test]
    fn new_host_with_password_writes_credential_and_stores_ref() {
        let (creds, service) = test_service();
        let hosts = service
            .save(&request_with_password("host-new", "secret123"))
            .unwrap();
        assert_eq!(hosts.len(), 1);
        let saved = &hosts[0];
        assert_eq!(
            saved.password_ref.as_deref(),
            Some("titanssh-host-new-password"),
            "落盘引用必须与写入 key 完全一致"
        );
        let entries = creds.entries();
        assert_eq!(
            entries
                .get("titanssh-host-new-password")
                .map(String::as_str),
            Some("secret123"),
            "明文凭据必须写入安全存储"
        );
    }

    #[test]
    fn new_host_without_password_has_no_ref_and_no_credential() {
        let (creds, service) = test_service();
        let hosts = service.save(&sample_request("host-new2", "prod")).unwrap();
        assert!(hosts[0].password_ref.is_none());
        assert!(creds.entries().is_empty(), "无凭据时不得写入安全存储");
    }

    #[test]
    fn new_host_with_passphrase_writes_credential() {
        let (creds, service) = test_service();
        let req = SaveHostRequest {
            auth_type: AuthType::PrivateKey,
            private_key_path: Some("~/.ssh/id_rsa".to_string()),
            passphrase: Some("pp-123".to_string()),
            ..sample_request("host-key", "prod")
        };
        let hosts = service.save(&req).unwrap();
        assert_eq!(
            hosts[0].passphrase_ref.as_deref(),
            Some("titanssh-host-key-passphrase")
        );
        assert!(hosts[0].password_ref.is_none());
        assert_eq!(
            creds
                .entries()
                .get("titanssh-host-key-passphrase")
                .map(String::as_str),
            Some("pp-123")
        );
    }

    #[test]
    fn edit_empty_password_preserves_old_ref() {
        let (creds, service) = test_service();
        service
            .save(&request_with_password("host-1", "secret123"))
            .unwrap();
        // 密码留空:应保留旧引用与旧凭据(P0-2 语义)
        let hosts = service.save(&request_with_password("host-1", "")).unwrap();
        assert_eq!(
            hosts[0].password_ref.as_deref(),
            Some("titanssh-host-1-password")
        );
        assert_eq!(
            creds
                .entries()
                .get("titanssh-host-1-password")
                .map(String::as_str),
            Some("secret123"),
            "留空不得覆盖旧凭据"
        );
    }

    #[test]
    fn edit_none_password_preserves_old_ref() {
        let (creds, service) = test_service();
        service
            .save(&request_with_password("host-1", "secret123"))
            .unwrap();
        let req = sample_request("host-1", "prod-updated"); // password: None
        let hosts = service.save(&req).unwrap();
        assert_eq!(
            hosts[0].password_ref.as_deref(),
            Some("titanssh-host-1-password"),
            "password 为 None 时应保留旧引用"
        );
        assert_eq!(
            creds
                .entries()
                .get("titanssh-host-1-password")
                .map(String::as_str),
            Some("secret123")
        );
    }

    #[test]
    fn edit_with_new_password_updates_credential_value() {
        let (creds, service) = test_service();
        service
            .save(&request_with_password("host-1", "secret123"))
            .unwrap();
        let hosts = service
            .save(&request_with_password("host-1", "new-secret"))
            .unwrap();
        assert_eq!(
            hosts[0].password_ref.as_deref(),
            Some("titanssh-host-1-password"),
            "覆盖写入仍使用派生 key"
        );
        assert_eq!(
            creds
                .entries()
                .get("titanssh-host-1-password")
                .map(String::as_str),
            Some("new-secret"),
            "新密码必须覆盖旧值"
        );
    }

    #[test]
    fn edit_updates_fields_in_place_without_duplicate() {
        let (_, service) = test_service();
        service
            .save(&request_with_password("host-1", "s1"))
            .unwrap();
        let req = SaveHostRequest {
            name: "renamed".to_string(),
            ..request_with_password("host-1", "s2")
        };
        let hosts = service.save(&req).unwrap();
        assert_eq!(hosts.len(), 1, "编辑不得产生重复条目");
        assert_eq!(hosts[0].name, "renamed");
    }

    // --- 保存:auth type 切换的陈旧凭据清理 ---

    #[test]
    fn auth_switch_to_private_key_deletes_stale_password() {
        let (creds, service) = test_service();
        service
            .save(&request_with_password("host-1", "secret123"))
            .unwrap();
        let req = SaveHostRequest {
            auth_type: AuthType::PrivateKey,
            private_key_path: Some("~/.ssh/id_rsa".to_string()),
            passphrase: Some("pp-123".to_string()),
            ..sample_request("host-1", "prod")
        };
        let hosts = service.save(&req).unwrap();
        assert!(hosts[0].password_ref.is_none(), "切换后不得保留旧密码引用");
        assert_eq!(
            hosts[0].passphrase_ref.as_deref(),
            Some("titanssh-host-1-passphrase")
        );
        let entries = creds.entries();
        assert!(entries.contains_key("titanssh-host-1-passphrase"));
        assert!(
            !entries.contains_key("titanssh-host-1-password"),
            "切换到私钥后旧密码凭据必须删除"
        );
    }

    #[test]
    fn auth_switch_to_password_deletes_stale_passphrase() {
        let (creds, service) = test_service();
        let privkey_req = SaveHostRequest {
            auth_type: AuthType::PrivateKey,
            private_key_path: Some("~/.ssh/id_rsa".to_string()),
            passphrase: Some("pp-123".to_string()),
            ..sample_request("host-2", "prod")
        };
        service.save(&privkey_req).unwrap();
        let req = request_with_password("host-2", "pwd-456");
        let hosts = service.save(&req).unwrap();
        assert!(
            hosts[0].passphrase_ref.is_none(),
            "切换后不得保留旧口令引用"
        );
        assert_eq!(
            hosts[0].password_ref.as_deref(),
            Some("titanssh-host-2-password")
        );
        let entries = creds.entries();
        assert!(entries.contains_key("titanssh-host-2-password"));
        assert!(
            !entries.contains_key("titanssh-host-2-passphrase"),
            "切换到密码后旧口令凭据必须删除"
        );
    }

    #[test]
    fn auth_switch_without_new_credential_drops_ref() {
        // Password → PrivateKey 但不提供口令:旧密码必须删除,口令引用为空
        let (creds, service) = test_service();
        service
            .save(&request_with_password("host-1", "secret123"))
            .unwrap();
        let req = SaveHostRequest {
            auth_type: AuthType::PrivateKey,
            private_key_path: Some("~/.ssh/id_rsa".to_string()),
            passphrase: None,
            ..sample_request("host-1", "prod")
        };
        let hosts = service.save(&req).unwrap();
        assert!(hosts[0].password_ref.is_none(), "切换时留空不得保留旧引用");
        assert!(hosts[0].passphrase_ref.is_none());
        assert!(
            !creds.entries().contains_key("titanssh-host-1-password"),
            "切换时旧密码凭据必须删除"
        );
    }

    // --- 保存:失败与补偿 ---

    #[test]
    fn file_write_failure_compensates_written_credentials() {
        let (creds, service) = unwritable_service();
        let result = service.save(&request_with_password("host-new", "secret123"));
        assert!(result.is_err(), "落盘失败必须返回错误");
        assert!(
            creds.entries().is_empty(),
            "落盘失败后必须补偿删除本次写入的凭据"
        );
    }

    #[test]
    fn credential_write_failure_leaves_file_untouched() {
        let (creds, service) = test_service();
        service
            .save(&request_with_password("host-1", "secret123"))
            .unwrap();
        creds.fail_set_for("titanssh-host-1-password");
        let result = service.save(&request_with_password("host-1", "new-secret"));
        assert!(result.is_err(), "凭据写入失败必须返回错误");
        // 文件仍为旧内容,旧凭据未被覆盖
        let hosts = service.list_hosts().unwrap();
        assert_eq!(
            hosts[0].password_ref.as_deref(),
            Some("titanssh-host-1-password")
        );
        assert_eq!(
            creds
                .entries()
                .get("titanssh-host-1-password")
                .map(String::as_str),
            Some("secret123"),
            "写入失败不得覆盖旧凭据"
        );
    }

    #[test]
    fn second_credential_write_failure_compensates_first() {
        // 请求同时携带密码与口令(非前端常规路径);第二个写入失败时补偿第一个
        let (creds, service) = test_service();
        let req = SaveHostRequest {
            private_key_path: Some("~/.ssh/id_rsa".to_string()),
            passphrase: Some("pp-123".to_string()),
            ..request_with_password("host-both", "pwd-123")
        };
        creds.fail_set_for("titanssh-host-both-passphrase");
        let result = service.save(&req);
        assert!(result.is_err());
        assert!(
            creds.entries().is_empty(),
            "第二个凭据写入失败后,已写入的第一个凭据必须补偿删除"
        );
    }

    // --- 删除 ---

    #[test]
    fn delete_removes_host_and_both_credentials() {
        let (creds, service) = test_service();
        service
            .save(&request_with_password("host-1", "secret123"))
            .unwrap();
        let hosts = service.delete("host-1").unwrap();
        assert!(hosts.is_empty());
        assert!(creds.entries().is_empty(), "删除后凭据必须清理");
        assert!(service.get_host("host-1").unwrap().is_none());
    }

    #[test]
    fn delete_file_failure_leaves_credentials_intact() {
        let (creds, service) = unwritable_service();
        // 预置凭据,模拟文件中存在该主机
        creds.set("titanssh-host-1-password", "secret123").unwrap();
        let result = service.delete("host-1");
        assert!(result.is_err(), "落盘失败必须返回错误");
        assert!(
            creds.entries().contains_key("titanssh-host-1-password"),
            "落盘失败时凭据不得被清理"
        );
    }

    #[test]
    fn delete_credential_failure_does_not_block_host_deletion() {
        let (creds, service) = test_service();
        service
            .save(&request_with_password("host-1", "secret123"))
            .unwrap();
        creds.fail_delete_for("titanssh-host-1-password");
        let hosts = service.delete("host-1").unwrap();
        assert!(hosts.is_empty(), "凭据删除失败不得阻断主机删除");
        assert!(
            creds.entries().contains_key("titanssh-host-1-password"),
            "删除失败保留孤儿条目(可接受)"
        );
    }

    // --- endpoint 信任记录生命周期清理 ---

    #[test]
    fn save_new_host_does_not_cleanup_trust() {
        let (_, service, cleanup) = test_service_with_cleanup();
        service.save(&sample_request("id1", "prod")).unwrap();
        assert!(
            cleanup.calls().is_empty(),
            "新建主机没有旧 endpoint，不得清理信任记录"
        );
    }

    #[test]
    fn endpoint_unchanged_field_edits_do_not_cleanup_trust() {
        let (_, service, cleanup) = test_service_with_cleanup();
        service.save(&request_with_password("id1", "s1")).unwrap();
        // 凭据变化：endpoint 不变
        service.save(&request_with_password("id1", "s2")).unwrap();
        // 名称、用户名、认证方式、分组、备注变化：endpoint 不变
        let req = SaveHostRequest {
            name: "renamed".to_string(),
            username: "admin".to_string(),
            auth_type: AuthType::PrivateKey,
            private_key_path: Some("~/.ssh/id".to_string()),
            passphrase: Some("pp-1".to_string()),
            remark: Some("note".to_string()),
            group: "grp".to_string(),
            ..sample_request("id1", "prod")
        };
        service.save(&req).unwrap();
        assert!(
            cleanup.calls().is_empty(),
            "endpoint 未变时不得清理信任记录"
        );
    }

    #[test]
    fn endpoint_host_edit_forgets_old_endpoint_when_last_reference() {
        let (_, service, cleanup) = test_service_with_cleanup();
        service.save(&sample_request("id1", "prod")).unwrap();
        let req = SaveHostRequest {
            host: "10.0.0.2".to_string(),
            ..sample_request("id1", "prod")
        };
        service.save(&req).unwrap();
        assert_eq!(
            cleanup.calls(),
            vec![("10.0.0.1".to_string(), 22)],
            "旧 endpoint 不再被引用时必须清理"
        );
    }

    #[test]
    fn endpoint_port_edit_forgets_old_port_endpoint() {
        let (_, service, cleanup) = test_service_with_cleanup();
        service.save(&sample_request("id1", "prod")).unwrap();
        let req = SaveHostRequest {
            port: 2222,
            ..sample_request("id1", "prod")
        };
        service.save(&req).unwrap();
        assert_eq!(
            cleanup.calls(),
            vec![("10.0.0.1".to_string(), 22)],
            "host 相同但端口不同就是不同 endpoint"
        );
    }

    #[test]
    fn endpoint_spelling_change_is_treated_as_endpoint_change() {
        // 精确 host 字符串语义：大小写变化视为不同 endpoint，不做归一化
        let (_, service, cleanup) = test_service_with_cleanup();
        let req = SaveHostRequest {
            host: "Prod.Example.COM".to_string(),
            ..sample_request("id1", "prod")
        };
        service.save(&req).unwrap();
        let req2 = SaveHostRequest {
            host: "prod.example.com".to_string(),
            ..req
        };
        service.save(&req2).unwrap();
        assert_eq!(cleanup.calls(), vec![("Prod.Example.COM".to_string(), 22)]);
    }

    #[test]
    fn endpoint_edit_keeps_record_when_other_host_still_references() {
        let (_, service, cleanup) = test_service_with_cleanup();
        service.save(&sample_request("id1", "prod")).unwrap();
        service.save(&sample_request("id2", "prod2")).unwrap();
        let req = SaveHostRequest {
            host: "10.0.0.2".to_string(),
            ..sample_request("id1", "prod")
        };
        service.save(&req).unwrap();
        assert!(
            cleanup.calls().is_empty(),
            "其他 HostConfig 仍引用旧 endpoint，不得清理共享记录"
        );
    }

    #[test]
    fn delete_forgets_endpoint_when_last_reference() {
        let (_, service, cleanup) = test_service_with_cleanup();
        service.save(&sample_request("id1", "prod")).unwrap();
        service.delete("id1").unwrap();
        assert_eq!(
            cleanup.calls(),
            vec![("10.0.0.1".to_string(), 22)],
            "删除最后一个引用后必须清理信任记录"
        );
    }

    #[test]
    fn delete_keeps_shared_endpoint_record_until_last_reference_gone() {
        let (_, service, cleanup) = test_service_with_cleanup();
        service.save(&sample_request("id1", "prod")).unwrap();
        service.save(&sample_request("id2", "prod2")).unwrap();
        service.delete("id1").unwrap();
        assert!(
            cleanup.calls().is_empty(),
            "删除共享 endpoint 的其中一个 HostConfig 不得清理"
        );
        service.delete("id2").unwrap();
        assert_eq!(
            cleanup.calls(),
            vec![("10.0.0.1".to_string(), 22)],
            "删除最后一个引用后才清理"
        );
    }

    #[test]
    fn delete_missing_host_does_not_cleanup() {
        let (_, service, cleanup) = test_service_with_cleanup();
        service.save(&sample_request("id1", "prod")).unwrap();
        service.delete("missing").unwrap();
        assert!(
            cleanup.calls().is_empty(),
            "不存在的 host 没有可清理的 endpoint"
        );
    }

    #[test]
    fn save_cleanup_failure_surfaces_structured_error_after_commit() {
        let (creds, service, cleanup) = test_service_with_cleanup();
        service.save(&request_with_password("id1", "s1")).unwrap();
        cleanup.fail_next();
        let req = SaveHostRequest {
            host: "10.0.0.2".to_string(),
            ..request_with_password("id1", "s2")
        };
        let error = service.save(&req).unwrap_err();
        assert_eq!(
            error.code(),
            "HostTrustCleanupFailed",
            "清理失败必须作为结构化错误显式返回"
        );
        assert!(error.to_string().contains("10.0.0.1:22"));
        // commit 已生效：配置已更新，已写入凭据不做补偿删除
        let hosts = service.list_hosts().unwrap();
        assert_eq!(hosts[0].host, "10.0.0.2");
        assert_eq!(
            creds
                .entries()
                .get("titanssh-id1-password")
                .map(String::as_str),
            Some("s2"),
            "commit 后的清理失败不得补偿删除凭据"
        );
    }

    #[test]
    fn delete_cleanup_failure_surfaces_structured_error_after_commit() {
        let (creds, service, cleanup) = test_service_with_cleanup();
        service.save(&request_with_password("id1", "s1")).unwrap();
        cleanup.fail_next();
        let error = service.delete("id1").unwrap_err();
        assert_eq!(error.code(), "HostTrustCleanupFailed");
        // 删除已生效，凭据按流程清理；但管理动作未完成必须显式报错
        assert!(service.list_hosts().unwrap().is_empty());
        assert!(
            !creds.entries().contains_key("titanssh-id1-password"),
            "删除后的凭据清理不受信任清理失败影响"
        );
    }

    /// commit 失败不触发清理：目录置为只读后 load 成功而落盘必然失败，
    /// 旧配置仍在（旧 endpoint 仍被引用），信任记录不得被动刀。
    #[cfg(unix)]
    #[test]
    fn save_commit_failure_does_not_attempt_cleanup() {
        use std::os::unix::fs::PermissionsExt;
        let (creds, service, file_path, cleanup) = test_service_with_path_and_cleanup();
        service.save(&request_with_password("id1", "s1")).unwrap();
        // 文件置为只读：load 成功而 commit 写入必然失败
        fs::set_permissions(&file_path, fs::Permissions::from_mode(0o444)).unwrap();
        let result = service.save(&SaveHostRequest {
            host: "10.0.0.2".to_string(),
            ..request_with_password("id1", "s2")
        });
        // 恢复权限，避免影响临时目录清理
        fs::set_permissions(&file_path, fs::Permissions::from_mode(0o644)).unwrap();

        assert!(result.is_err(), "落盘失败必须返回错误");
        assert!(
            cleanup.calls().is_empty(),
            "commit 未成功不得尝试清理信任记录"
        );
        assert!(creds.entries().is_empty(), "落盘失败时补偿删除写入的凭据");
    }

    // --- 集成：HostConfig 生命周期 × 真实信任存储与身份服务 ---

    /// 构造真实 TrustStore + HostIdentityService 并预置一条信任记录
    fn identity_with_record(record: TrustRecord) -> (HostIdentityService, PathBuf) {
        let dir = std::env::temp_dir().join(format!("titan-host-trust-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).expect("temp dir should be created");
        let path = dir.join("known_hosts");
        let store = TrustStore::from_file_path(path.clone());
        store.upsert(record).expect("预置记录应写入成功");
        (HostIdentityService::with_trust_store(store), path)
    }

    /// 构造注入真实身份服务（共享同一 TrustStore 实例）的 HostConfigService
    fn service_with_identity(
        identity: &HostIdentityService,
    ) -> (Arc<MemoryCredentialStore>, HostConfigService) {
        let credentials = Arc::new(MemoryCredentialStore::new());
        let store = HostStore::from_file_path(temp_hosts_file());
        let cleanup = IdentityTrustCleanup {
            identity_service: identity.clone(),
        };
        let service =
            HostConfigService::with_stores(store, Box::new(credentials.clone()), Box::new(cleanup));
        (credentials, service)
    }

    /// 构造校验呈现信息（指纹由 blob 派生）
    fn presented(host: &str, port: u16, blob: &[u8]) -> PresentedHostKey {
        PresentedHostKey {
            host: host.to_string(),
            port,
            algorithm: "ssh-ed25519".to_string(),
            fingerprint: crate::core::host_identity::fingerprint_sha256(blob),
            blob: blob.to_vec(),
        }
    }

    /// 等待指定 Session 出现 pending challenge（超时则 panic）
    fn wait_pending(service: &HostIdentityService, session_id: &str) -> HostIdentityChallenge {
        let deadline = Instant::now() + Duration::from_secs(2);
        while service.pending_challenge(session_id).is_none() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        service
            .pending_challenge(session_id)
            .expect("challenge 已创建")
    }

    /// endpoint 编辑（最后引用）：旧记录从磁盘删除，新 Session 将旧 endpoint
    /// 视为未知并重新触发确认。
    #[test]
    fn integration_endpoint_edit_removes_trust_record_and_new_sessions_prompt() {
        let app = tauri::test::mock_app();
        let (identity, path) = identity_with_record(TrustRecord {
            host: "10.0.0.1".to_string(),
            port: 22,
            algorithm: "ssh-ed25519".to_string(),
            blob: b"blob".to_vec(),
        });
        let (_, service) = service_with_identity(&identity);
        service.save(&sample_request("id1", "prod")).unwrap();
        service
            .save(&SaveHostRequest {
                host: "10.0.0.2".to_string(),
                ..sample_request("id1", "prod")
            })
            .unwrap();

        // 磁盘真相：旧 endpoint 记录已删除
        assert_eq!(
            TrustStore::from_file_path(path)
                .lookup("10.0.0.1", 22)
                .unwrap(),
            None
        );
        // 新 Session 将旧 endpoint 视为未知并重新确认
        let verifier = identity.verifier(app.handle().clone(), "session-1".to_string());
        let presented_key = presented("10.0.0.1", 22, b"blob");
        let waiter = thread::spawn(move || verifier(&presented_key));
        let challenge = wait_pending(&identity, "session-1");
        identity.reject(&challenge.challenge_id).unwrap();
        assert_eq!(
            waiter.join().unwrap().unwrap_err().code(),
            "HostKeyRejected"
        );
    }

    /// 重复引用：删除其中一个 HostConfig 保留共享记录，删除最后一个引用才清理。
    #[test]
    fn integration_duplicate_endpoint_keeps_shared_record_until_last_delete() {
        let (identity, path) = identity_with_record(TrustRecord {
            host: "10.0.0.1".to_string(),
            port: 22,
            algorithm: "ssh-ed25519".to_string(),
            blob: b"blob".to_vec(),
        });
        let (_, service) = service_with_identity(&identity);
        service.save(&sample_request("id1", "prod-a")).unwrap();
        service.save(&sample_request("id2", "prod-b")).unwrap();

        service.delete("id1").unwrap();
        assert!(
            TrustStore::from_file_path(path.clone())
                .lookup("10.0.0.1", 22)
                .unwrap()
                .is_some(),
            "删除共享 endpoint 的其中一个 HostConfig 不得清理记录"
        );
        service.delete("id2").unwrap();
        assert_eq!(
            TrustStore::from_file_path(path)
                .lookup("10.0.0.1", 22)
                .unwrap(),
            None,
            "删除最后一个引用后才清理"
        );
    }

    /// endpoint 未变的编辑（用户名等字段）保留信任记录。
    #[test]
    fn integration_unchanged_endpoint_edit_keeps_trust_record() {
        let (identity, path) = identity_with_record(TrustRecord {
            host: "10.0.0.1".to_string(),
            port: 22,
            algorithm: "ssh-ed25519".to_string(),
            blob: b"blob".to_vec(),
        });
        let (_, service) = service_with_identity(&identity);
        service.save(&sample_request("id1", "prod")).unwrap();
        service
            .save(&SaveHostRequest {
                name: "renamed".to_string(),
                username: "admin".to_string(),
                ..sample_request("id1", "prod")
            })
            .unwrap();
        assert!(
            TrustStore::from_file_path(path)
                .lookup("10.0.0.1", 22)
                .unwrap()
                .is_some(),
            "endpoint 未变不得清理信任记录"
        );
    }

    /// 已通过持久化匹配静默验证的活动 Session：清理后同 Session 重连仍静默放行，
    /// 新 Session 重新确认旧 endpoint。
    #[test]
    fn integration_cleanup_preserves_silently_verified_session_decision() {
        let app = tauri::test::mock_app();
        let (identity, path) = identity_with_record(TrustRecord {
            host: "10.0.0.1".to_string(),
            port: 22,
            algorithm: "ssh-ed25519".to_string(),
            blob: b"persisted".to_vec(),
        });
        // 活动 Session 经持久化匹配静默验证
        let presented_key = presented("10.0.0.1", 22, b"persisted");
        let verifier = identity.verifier(app.handle().clone(), "session-1".to_string());
        verifier(&presented_key).expect("持久化匹配静默放行");

        // 编辑 endpoint 触发信任清理
        let (_, service) = service_with_identity(&identity);
        service.save(&sample_request("id1", "prod")).unwrap();
        service
            .save(&SaveHostRequest {
                host: "10.0.0.2".to_string(),
                ..sample_request("id1", "prod")
            })
            .unwrap();
        assert_eq!(
            TrustStore::from_file_path(path)
                .lookup("10.0.0.1", 22)
                .unwrap(),
            None
        );

        // 已验证决定持续到 Session 关闭：同 Session 重连仍静默放行
        verifier(&presented_key).expect("已验证决定持续到 Session 关闭");
        assert!(identity.pending_challenge("session-1").is_none());
        // 新 Session 将旧 endpoint 视为未知并重新确认
        let verifier_b = identity.verifier(app.handle().clone(), "session-b".to_string());
        let waiter_b = thread::spawn(move || verifier_b(&presented_key));
        let challenge_b = wait_pending(&identity, "session-b");
        identity.reject(&challenge_b.challenge_id).unwrap();
        assert_eq!(
            waiter_b.join().unwrap().unwrap_err().code(),
            "HostKeyRejected"
        );
    }

    /// 清理不干扰运行中的 Runtime Session：临时信任持续到 Session 关闭，
    /// 新 Session 重新确认旧 endpoint。
    #[test]
    fn integration_cleanup_does_not_disturb_active_session() {
        let app = tauri::test::mock_app();
        let (identity, path) = identity_with_record(TrustRecord {
            host: "10.0.0.1".to_string(),
            port: 22,
            algorithm: "ssh-ed25519".to_string(),
            blob: b"persisted".to_vec(),
        });
        // 活动 Session 呈现不同 key：challenge → 仅本次接受（临时信任）
        let presented_key = presented("10.0.0.1", 22, b"rotated");
        let verifier = identity.verifier(app.handle().clone(), "session-1".to_string());
        let waiter = thread::spawn({
            let verifier = verifier.clone();
            let presented_key = presented_key.clone();
            move || verifier(&presented_key)
        });
        let challenge = wait_pending(&identity, "session-1");
        identity.accept(&challenge.challenge_id).unwrap();
        waiter.join().unwrap().unwrap();

        // 编辑 endpoint 触发信任清理
        let (_, service) = service_with_identity(&identity);
        service.save(&sample_request("id1", "prod")).unwrap();
        service
            .save(&SaveHostRequest {
                host: "10.0.0.2".to_string(),
                ..sample_request("id1", "prod")
            })
            .unwrap();
        assert_eq!(
            TrustStore::from_file_path(path)
                .lookup("10.0.0.1", 22)
                .unwrap(),
            None
        );

        // 清理不关闭也不影响运行中的 Session：重连仍放行
        verifier(&presented_key).expect("活动 Session 的临时信任持续到关闭");
        // 新 Session 将旧 endpoint 视为未知并重新确认
        let verifier_b = identity.verifier(app.handle().clone(), "session-b".to_string());
        let waiter_b = thread::spawn(move || verifier_b(&presented_key));
        let challenge_b = wait_pending(&identity, "session-b");
        identity.reject(&challenge_b.challenge_id).unwrap();
        assert_eq!(
            waiter_b.join().unwrap().unwrap_err().code(),
            "HostKeyRejected"
        );
    }

    /// 真实信任存储写盘失败：清理错误经真实 seam 以结构化代码返回，
    /// 配置变更本身已生效。
    #[test]
    fn integration_cleanup_failure_propagates_structured_error() {
        let (identity, path) = identity_with_record(TrustRecord {
            host: "10.0.0.1".to_string(),
            port: 22,
            algorithm: "ssh-ed25519".to_string(),
            blob: b"blob".to_vec(),
        });
        let (_, service) = service_with_identity(&identity);
        service.save(&sample_request("id1", "prod")).unwrap();
        // 破坏发布目标：known_hosts 路径替换为目录（缓存已加载，写入必然失败）
        fs::remove_file(&path).unwrap();
        fs::create_dir_all(&path).unwrap();

        let error = service
            .save(&SaveHostRequest {
                host: "10.0.0.2".to_string(),
                ..sample_request("id1", "prod")
            })
            .unwrap_err();
        assert_eq!(error.code(), "HostTrustCleanupFailed");
        // 配置变更已生效
        assert_eq!(service.list_hosts().unwrap()[0].host, "10.0.0.2");
    }

    // --- 查询 ---

    #[test]
    fn get_host_returns_saved_host() {
        let (_, service) = test_service();
        service
            .save(&request_with_password("host-1", "secret123"))
            .unwrap();
        let host = service.get_host("host-1").unwrap();
        assert_eq!(host.unwrap().id, "host-1");
    }

    #[test]
    fn get_host_returns_none_for_missing() {
        let (_, service) = test_service();
        assert!(service.get_host("missing").unwrap().is_none());
    }

    #[test]
    fn list_hosts_returns_saved_hosts() {
        let (_, service) = test_service();
        service
            .save(&request_with_password("host-1", "s1"))
            .unwrap();
        service
            .save(&request_with_password("host-2", "s2"))
            .unwrap();
        let hosts = service.list_hosts().unwrap();
        assert_eq!(hosts.len(), 2);
        assert!(hosts.iter().all(|h| h.password_ref.is_some()));
    }

    // --- 无明文不变量(原 host_store 复制逻辑 proptest 迁移,改为真实 composition) ---

    /// 生成非空字符串的策略(至少1个可打印字符,最多64个字符)
    fn arb_nonempty_string() -> impl Strategy<Value = String> {
        "[a-zA-Z0-9_\\-\\.]{1,64}".prop_map(|s| s)
    }

    /// 生成非空凭据字符串的策略(至少8个字符,最多24个字符,纯小写字母)
    /// 使用固定前缀 "TESTPWD__" 确保凭据字符串足够独特,不会与其他字段值产生误匹配
    fn arb_credential_string() -> impl Strategy<Value = String> {
        "[a-z]{8,24}".prop_map(|s| format!("TESTPWD__{}", s))
    }

    /// 生成含明文密码的 SaveHostRequest 策略(密码认证模式)
    fn arb_password_save_request() -> impl Strategy<Value = SaveHostRequest> {
        (
            arb_nonempty_string(),
            arb_nonempty_string(),
            arb_nonempty_string(),
            1u16..=65535u16,
            arb_nonempty_string(),
            arb_credential_string(),
        )
            .prop_map(
                |(id, name, host, port, username, password)| SaveHostRequest {
                    id,
                    name,
                    host,
                    port,
                    username,
                    auth_type: AuthType::Password,
                    password: Some(password),
                    private_key_path: None,
                    passphrase: None,
                    remark: None,
                    group: String::new(),
                },
            )
    }

    /// 生成含明文口令的 SaveHostRequest 策略(私钥认证模式)
    fn arb_passphrase_save_request() -> impl Strategy<Value = SaveHostRequest> {
        (
            arb_nonempty_string(),
            arb_nonempty_string(),
            arb_nonempty_string(),
            1u16..=65535u16,
            arb_nonempty_string(),
            arb_nonempty_string(),
            arb_credential_string(),
        )
            .prop_map(
                |(id, name, host, port, username, private_key_path, passphrase)| SaveHostRequest {
                    id,
                    name,
                    host,
                    port,
                    username,
                    auth_type: AuthType::PrivateKey,
                    password: None,
                    private_key_path: Some(private_key_path),
                    passphrase: Some(passphrase),
                    remark: None,
                    group: String::new(),
                },
            )
    }

    /// 生成同时含明文密码和口令的 SaveHostRequest 策略
    fn arb_both_credentials_save_request() -> impl Strategy<Value = SaveHostRequest> {
        (
            arb_nonempty_string(),
            arb_nonempty_string(),
            arb_nonempty_string(),
            1u16..=65535u16,
            arb_nonempty_string(),
            arb_credential_string(),
            arb_nonempty_string(),
            arb_credential_string(),
        )
            .prop_map(
                |(id, name, host, port, username, password, private_key_path, passphrase)| {
                    SaveHostRequest {
                        id,
                        name,
                        host,
                        port,
                        username,
                        auth_type: AuthType::Password,
                        password: Some(password),
                        private_key_path: Some(private_key_path),
                        passphrase: Some(passphrase),
                        remark: None,
                        group: String::new(),
                    }
                },
            )
    }

    proptest! {
        /// **验证: hosts.json 不含明文密码(密码认证模式,真实 composition)**
        #[test]
        fn prop_hosts_json_no_plaintext_password(request in arb_password_save_request()) {
            let (_, service, file_path) = test_service_with_path();
            service.save(&request).expect("save 应成功");

            let raw_content = fs::read_to_string(&file_path).expect("hosts.json 应可读取");
            let plaintext = request.password.clone().unwrap();
            prop_assert!(
                !raw_content.contains(&plaintext),
                "hosts.json 不得包含明文密码,密码: {:?}",
                plaintext
            );
        }

        /// **验证: hosts.json 不含明文口令(私钥口令模式,真实 composition)**
        #[test]
        fn prop_hosts_json_no_plaintext_passphrase(request in arb_passphrase_save_request()) {
            let (_, service, file_path) = test_service_with_path();
            service.save(&request).expect("save 应成功");

            let raw_content = fs::read_to_string(&file_path).expect("hosts.json 应可读取");
            let plaintext = request.passphrase.clone().unwrap();
            prop_assert!(
                !raw_content.contains(&plaintext),
                "hosts.json 不得包含明文口令,口令: {:?}",
                plaintext
            );
        }

        /// **验证: hosts.json 不含明文凭据(密码与口令同时存在,真实 composition)**
        #[test]
        fn prop_hosts_json_no_plaintext_both_credentials(request in arb_both_credentials_save_request()) {
            let (_, service, file_path) = test_service_with_path();
            service.save(&request).expect("save 应成功");

            let raw_content = fs::read_to_string(&file_path).expect("hosts.json 应可读取");
            let plaintext_password = request.password.clone().unwrap();
            let plaintext_passphrase = request.passphrase.clone().unwrap();
            prop_assert!(
                !raw_content.contains(&plaintext_password),
                "hosts.json 不得包含明文密码,密码: {:?}",
                plaintext_password
            );
            prop_assert!(
                !raw_content.contains(&plaintext_passphrase),
                "hosts.json 不得包含明文口令,口令: {:?}",
                plaintext_passphrase
            );
        }

        /// **验证: name/host/username 任一空白时保存被拒绝(真实 composition 入口)**
        #[test]
        fn prop_invalid_host_config_rejected(
            id in arb_nonempty_string(),
            blank in prop_oneof![
                Just(String::new()),
                " {1,8}".prop_map(|s| s),
                "\t{1,4}".prop_map(|s| s),
                "[ \t]{1,8}".prop_map(|s| s),
            ],
            valid in arb_nonempty_string(),
        ) {
            // 轮流将 name/host/username 置为空白
            for (name, host, username) in [
                (blank.clone(), valid.clone(), valid.clone()),
                (valid.clone(), blank.clone(), valid.clone()),
                (valid.clone(), valid.clone(), blank.clone()),
            ] {
                let request = SaveHostRequest {
                    id: id.clone(),
                    name,
                    host,
                    port: 22,
                    username,
                    auth_type: AuthType::Password,
                    password: None,
                    private_key_path: None,
                    passphrase: None,
                    remark: None,
                    group: String::new(),
                };
                let (_, service) = test_service();
                prop_assert!(
                    service.save(&request).is_err(),
                    "name/host/username 为空白时 save 必须返回 Err"
                );
            }
        }
    }
}
