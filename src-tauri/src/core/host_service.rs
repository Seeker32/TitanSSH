use crate::errors::app_error::AppError;
use crate::models::host::{AuthType, HostConfig, SaveHostRequest};
use crate::storage::host_store::HostStore;
use crate::storage::secure_store;
use tauri::AppHandle;

/// 凭据存储 adapter seam:HostConfig 持久化 module 只依赖此 trait 访问 OS 安全存储
///
/// 真实实现包装 secure_store(Keychain / Credential Manager / Secret Service);
/// 内存实现仅用于测试,可注入失败以覆盖错误路径。
pub trait CredentialStore {
    /// 写入凭据;写入失败时上层负责补偿
    fn set(&self, key: &str, value: &str) -> Result<(), AppError>;

    /// 删除凭据;不存在的 key 静默成功
    fn delete(&self, key: &str) -> Result<(), AppError>;
}

/// 真实凭据存储:包装 OS secure storage
pub struct SecureCredentialStore;

impl CredentialStore for SecureCredentialStore {
    fn set(&self, key: &str, value: &str) -> Result<(), AppError> {
        secure_store::set_credential(key, value)
    }

    fn delete(&self, key: &str) -> Result<(), AppError> {
        secure_store::delete_credential(key)
    }
}

/// HostConfig 持久化 deep module
///
/// 拥有主机配置保存/删除的完整业务流程:请求校验、凭据写入与引用解析、
/// auth type 切换的陈旧凭据清理,以及失败补偿。hosts.json(HostStore)与
/// OS 安全存储(CredentialStore)是内部 adapter,不向 command 层泄漏。
pub struct HostConfigService {
    host_store: HostStore,
    credential_store: Box<dyn CredentialStore>,
}

impl HostConfigService {
    /// 生产构造:从 AppHandle 内建文件存储与真实凭据存储
    ///
    /// # 参数
    /// - `app`: Tauri 应用句柄,用于解析 hosts.json 所在的应用数据目录
    ///
    /// # 副作用
    /// 解析并创建应用数据目录;失败返回 StorageError
    pub fn new(app: &AppHandle) -> Result<Self, AppError> {
        Ok(Self {
            host_store: HostStore::new(app)?,
            credential_store: Box::new(SecureCredentialStore),
        })
    }

    /// 测试构造:注入文件存储与凭据存储,覆盖成功与失败路径
    #[cfg(test)]
    fn with_stores(host_store: HostStore, credential_store: Box<dyn CredentialStore>) -> Self {
        Self {
            host_store,
            credential_store,
        }
    }

    /// 列出所有已保存的主机配置,不含明文凭据
    pub fn list_hosts(&self) -> Result<Vec<HostConfig>, AppError> {
        self.host_store.load()
    }

    /// 按 id 查询主机配置;不存在时返回 None
    ///
    /// # 参数
    /// - `host_id`: 目标主机的唯一标识符
    pub fn get_host(&self, host_id: &str) -> Result<Option<HostConfig>, AppError> {
        let hosts = self.host_store.load()?;
        Ok(hosts.into_iter().find(|host| host.id == host_id))
    }

    /// 保存主机配置,返回更新后的完整列表
    ///
    /// 流程:校验 → 加载现有列表 → 写入新凭据并解析引用(留空/缺省且 auth type
    /// 未变时保留旧引用)→ 构造 HostConfig → upsert → 落盘(commit 点)→
    /// auth type 切换时尽力删除陈旧凭据。
    ///
    /// # 一致性保证
    /// 任一失败(凭据写入或落盘)时,补偿删除本次调用已写入的凭据,保持
    /// 安全存储与 hosts.json 一致;陈旧凭据清理只发生在 commit 之后,
    /// 失败时旧凭据保持可用。
    ///
    /// # 参数
    /// - `request`: 含明文凭据的保存请求,处理完毕后明文不持久化
    pub fn save(&self, request: &SaveHostRequest) -> Result<Vec<HostConfig>, AppError> {
        validate_save_request(request)?;

        // 本次调用已写入的凭据 key;任一失败时用于补偿删除,保持安全存储与文件一致
        let mut written_keys: Vec<String> = Vec::new();

        let result = (|| -> Result<Vec<HostConfig>, AppError> {
            let existing_hosts = self.host_store.load()?;
            let existing = existing_hosts.iter().find(|host| host.id == request.id);
            // 认证方式切换时,旧凭据不再适用,不得保留其引用
            let auth_type_changed = existing
                .map(|host| host.auth_type != request.auth_type)
                .unwrap_or(false);

            let password_ref = self.resolve_credential_ref(
                &request.id,
                &request.password,
                secure_store::password_key,
                existing.and_then(|host| host.password_ref.as_deref()),
                auth_type_changed,
                &mut written_keys,
            )?;

            let passphrase_ref = self.resolve_credential_ref(
                &request.id,
                &request.passphrase,
                secure_store::passphrase_key,
                existing.and_then(|host| host.passphrase_ref.as_deref()),
                auth_type_changed,
                &mut written_keys,
            )?;

            // 构建不含明文的 HostConfig 用于落盘
            let host_config = HostConfig {
                id: request.id.clone(),
                name: request.name.clone(),
                host: request.host.clone(),
                port: request.port,
                username: request.username.clone(),
                auth_type: request.auth_type.clone(),
                password_ref,
                private_key_path: request.private_key_path.clone(),
                passphrase_ref,
                remark: request.remark.clone(),
            };

            // 复用已加载的主机列表,避免重复读取文件
            let mut hosts = existing_hosts;
            if let Some(index) = hosts.iter().position(|item| item.id == host_config.id) {
                hosts[index] = host_config;
            } else {
                hosts.push(host_config);
            }

            // 落盘是 commit 点:文件更新成功后,才清理切换遗留的陈旧凭据
            self.host_store.save(&hosts)?;
            if auth_type_changed {
                if request.auth_type == AuthType::PrivateKey {
                    // 旧密码不再适用,尽力删除(失败只留孤儿条目,不阻断保存)
                    let _ = self
                        .credential_store
                        .delete(&secure_store::password_key(&request.id));
                } else {
                    // 旧口令不再适用
                    let _ = self
                        .credential_store
                        .delete(&secure_store::passphrase_key(&request.id));
                }
            }
            Ok(hosts)
        })();

        match result {
            Ok(hosts) => Ok(hosts),
            Err(error) => {
                // 失败补偿:删除本次调用写入的凭据,避免孤儿条目与悬空引用
                for key in &written_keys {
                    let _ = self.credential_store.delete(key);
                }
                Err(error)
            }
        }
    }

    /// 删除主机配置,返回更新后的完整列表
    ///
    /// 先落盘移除条目(commit 点),成功后再尽力删除密码与口令两个凭据 key;
    /// 凭据删除失败不阻断主机删除(最多留下孤儿 keychain 条目)。
    ///
    /// # 参数
    /// - `host_id`: 要删除的主机 ID
    pub fn delete(&self, host_id: &str) -> Result<Vec<HostConfig>, AppError> {
        let mut hosts = self.host_store.load()?;
        hosts.retain(|host| host.id != host_id);

        // 落盘是 commit 点:失败时凭据保持原样
        self.host_store.save(&hosts)?;

        // 无条件清理两个凭据 key(幂等,兼容切换清理上线前的遗留数据)
        let _ = self
            .credential_store
            .delete(&secure_store::password_key(host_id));
        let _ = self
            .credential_store
            .delete(&secure_store::passphrase_key(host_id));
        Ok(hosts)
    }

    /// 解析单个凭据引用:非空明文写入安全存储并返回新引用 key;
    /// 空/缺省时,认证方式未切换则保留旧引用,切换则置 None
    ///
    /// # 参数
    /// - `host_id`: 主机 ID,用于派生引用 key
    /// - `provided`: 请求中的明文凭据(可能为空串)
    /// - `key_fn`: 引用 key 生成函数(password_key / passphrase_key)
    /// - `existing_ref`: 旧引用
    /// - `auth_type_changed`: 认证方式是否已切换
    /// - `written_keys`: 本次调用已写入的 key 列表,供失败补偿
    fn resolve_credential_ref(
        &self,
        host_id: &str,
        provided: &Option<String>,
        key_fn: fn(&str) -> String,
        existing_ref: Option<&str>,
        auth_type_changed: bool,
        written_keys: &mut Vec<String>,
    ) -> Result<Option<String>, AppError> {
        if let Some(value) = provided {
            if !value.is_empty() {
                let key = key_fn(host_id);
                self.credential_store.set(&key, value)?;
                written_keys.push(key.clone());
                return Ok(Some(key));
            }
        }
        if auth_type_changed {
            // 切换认证方式:旧引用不再适用,置 None
            Ok(None)
        } else {
            Ok(existing_ref.map(String::from))
        }
    }
}

/// 验证保存主机请求的必填字段,name/host/username 不得为空白
fn validate_save_request(request: &SaveHostRequest) -> Result<(), AppError> {
    if request.name.trim().is_empty() {
        return Err(AppError::InvalidHostConfig("主机名称为必填项".to_string()));
    }
    if request.host.trim().is_empty() {
        return Err(AppError::InvalidHostConfig("主机地址为必填项".to_string()));
    }
    if request.username.trim().is_empty() {
        return Err(AppError::InvalidHostConfig("用户名为必填项".to_string()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use std::collections::HashMap;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};
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

    /// 内存凭据存储:记录写入/删除结果,可针对特定 key 注入失败
    #[derive(Default)]
    struct MemoryCredentialStore {
        entries: Mutex<HashMap<String, String>>,
        fail_set_key: Mutex<Option<String>>,
        fail_delete_key: Mutex<Option<String>>,
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
                return Err(AppError::SecureStoreError("注入的写入失败".to_string()));
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
                return Err(AppError::SecureStoreError("注入的删除失败".to_string()));
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

    /// 构造测试服务,并返回 hosts.json 路径供原始内容断言
    fn test_service_with_path() -> (Arc<MemoryCredentialStore>, HostConfigService, PathBuf) {
        let credentials = Arc::new(MemoryCredentialStore::new());
        let file_path = temp_hosts_file();
        let store = HostStore::from_file_path(file_path.clone());
        let service = HostConfigService::with_stores(store, Box::new(credentials.clone()));
        (credentials, service, file_path)
    }

    /// 构造指向不可写目录的测试服务
    fn unwritable_service() -> (Arc<MemoryCredentialStore>, HostConfigService) {
        let credentials = Arc::new(MemoryCredentialStore::new());
        let store = HostStore::from_file_path(unwritable_hosts_file());
        let service = HostConfigService::with_stores(store, Box::new(credentials.clone()));
        (credentials, service)
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
