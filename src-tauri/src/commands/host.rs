use crate::errors::app_error::AppError;
use crate::models::host::{AuthType, HostConfig, SaveHostRequest};
use crate::storage::host_store::HostStore;
use crate::storage::secure_store;
use tauri::AppHandle;

/// 列出所有已保存的主机配置，不含明文凭据
#[tauri::command]
pub fn list_hosts(app: AppHandle) -> Result<Vec<HostConfig>, String> {
    let store = HostStore::new(&app)?;
    store.load().map_err(String::from)
}

/// 保存主机配置：将明文凭据写入 OS 安全存储，仅将引用键落盘
/// - request: 含明文凭据的保存请求，处理完毕后明文不得持久化
/// - 返回更新后的主机列表
#[tauri::command]
pub fn save_host(app: AppHandle, request: SaveHostRequest) -> Result<Vec<HostConfig>, String> {
    validate_save_request(&request)?;

    // 加载现有主机列表，用于编辑时保留旧凭据引用
    let store = HostStore::new(&app)?;
    let existing_hosts = store.load()?;
    let existing = existing_hosts.iter().find(|h| h.id == request.id);

    // 将明文凭据写入安全存储，生成引用键
    // 若密码为空且为编辑模式，则保留旧引用键（"留空则保持原密码不变"）
    let password_ref = if let Some(ref pwd) = request.password {
        if !pwd.is_empty() {
            // 使用统一的 key 生成函数，确保写入 key 与落盘引用值完全一致（修复 P0-1）
            let key = secure_store::password_key(&request.id);
            secure_store::set_credential(&key, pwd)
                .map_err(|e| String::from(e))?;
            Some(key)
        } else {
            // 密码留空：编辑时保留旧引用，新建时置 None
            existing.and_then(|h| h.password_ref.clone())
        }
    } else {
        existing.and_then(|h| h.password_ref.clone())
    };

    // 若口令为空且为编辑模式，则保留旧引用键（"留空则保持原口令不变"）
    let passphrase_ref = if let Some(ref pp) = request.passphrase {
        if !pp.is_empty() {
            let key = secure_store::passphrase_key(&request.id);
            secure_store::set_credential(&key, pp)
                .map_err(|e| String::from(e))?;
            Some(key)
        } else {
            existing.and_then(|h| h.passphrase_ref.clone())
        }
    } else {
        existing.and_then(|h| h.passphrase_ref.clone())
    };

    // 构建不含明文的 HostConfig 用于落盘
    let host_config = HostConfig {
        id: request.id,
        name: request.name,
        host: request.host,
        port: request.port,
        username: request.username,
        auth_type: request.auth_type,
        password_ref,
        private_key_path: request.private_key_path,
        passphrase_ref,
        remark: request.remark,
    };

    // 复用已加载的主机列表，避免重复读取文件
    let mut hosts = existing_hosts;

    if let Some(index) = hosts.iter().position(|item| item.id == host_config.id) {
        hosts[index] = host_config;
    } else {
        hosts.push(host_config);
    }

    store.save(&hosts)?;
    Ok(hosts)
}

/// 删除主机配置，同步清理 OS 安全存储中的凭据
/// - host_id: 要删除的主机 ID
/// - 返回更新后的主机列表
#[tauri::command]
pub fn delete_host(app: AppHandle, host_id: String) -> Result<Vec<HostConfig>, String> {
    let store = HostStore::new(&app)?;
    let mut hosts = store.load()?;

    // 删除前清理安全存储中的凭据，使用统一 key 生成函数确保格式一致
    if let Some(host) = hosts.iter().find(|h| h.id == host_id) {
        if host.auth_type == AuthType::Password {
            let _ = secure_store::delete_credential(&secure_store::password_key(&host_id));
        }
        let _ = secure_store::delete_credential(&secure_store::passphrase_key(&host_id));
    }

    hosts.retain(|host| host.id != host_id);
    store.save(&hosts)?;
    Ok(hosts)
}

/// 验证保存主机请求的必填字段，name/host/username 不得为空
fn validate_save_request(request: &SaveHostRequest) -> Result<(), String> {
    if request.name.trim().is_empty() {
        return Err(String::from(AppError::InvalidHostConfig(
            "Host name is required".to_string(),
        )));
    }
    if request.host.trim().is_empty() {
        return Err(String::from(AppError::InvalidHostConfig(
            "Host address is required".to_string(),
        )));
    }
    if request.username.trim().is_empty() {
        return Err(String::from(AppError::InvalidHostConfig(
            "Username is required".to_string(),
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::validate_save_request;
    use crate::models::host::{AuthType, HostConfig, SaveHostRequest};
    use crate::storage::secure_store;
    use proptest::prelude::*;
    use uuid::Uuid;

    /// 生成空白字符串的策略：空字符串或仅含空格/制表符的字符串
    fn arb_blank_string() -> impl Strategy<Value = String> {
        prop_oneof![
            Just(String::new()),
            " {1,8}".prop_map(|s| s),
            "\t{1,4}".prop_map(|s| s),
            "[ \t]{1,8}".prop_map(|s| s),
        ]
    }

    /// 生成非空合法字符串的策略
    fn arb_valid_string() -> impl Strategy<Value = String> {
        "[a-zA-Z0-9_\\-\\.]{1,32}".prop_map(|s| s)
    }

    /// 生成至少一个必填字段（name/host/username）为空白的 SaveHostRequest 策略
    fn arb_invalid_save_request() -> impl Strategy<Value = SaveHostRequest> {
        (0usize..3, arb_blank_string(), arb_valid_string(), arb_valid_string())
            .prop_flat_map(|(blank_field, blank, valid_a, valid_b)| {
                let (name, host, username) = match blank_field {
                    0 => (blank, valid_a, valid_b),
                    1 => (valid_a, blank, valid_b),
                    _ => (valid_a, valid_b, blank),
                };
                (Just(name), Just(host), Just(username))
            })
            .prop_map(|(name, host, username)| SaveHostRequest {
                id: Uuid::new_v4().to_string(),
                name,
                host,
                port: 22,
                username,
                auth_type: AuthType::Password,
                password: None,
                private_key_path: None,
                passphrase: None,
                remark: None,
            })
    }

    proptest! {
        /// **验证: 需求 1.2** — 无效主机配置被拒绝
        #[test]
        fn prop_invalid_host_config_rejected(request in arb_invalid_save_request()) {
            let result = validate_save_request(&request);
            prop_assert!(
                result.is_err(),
                "name/host/username 为空白时，validate_save_request 必须返回 Err；\
                 name={:?}, host={:?}, username={:?}",
                request.name, request.host, request.username
            );
        }
    }

    #[test]
    fn validate_rejects_blank_name() {
        let req = SaveHostRequest {
            id: "id1".to_string(),
            name: "   ".to_string(),
            host: "10.0.0.1".to_string(),
            port: 22,
            username: "root".to_string(),
            auth_type: AuthType::Password,
            password: None,
            private_key_path: None,
            passphrase: None,
            remark: None,
        };
        assert!(validate_save_request(&req).is_err());
    }

    #[test]
    fn validate_rejects_blank_host() {
        let req = SaveHostRequest {
            id: "id2".to_string(),
            name: "prod".to_string(),
            host: "\t".to_string(),
            port: 22,
            username: "root".to_string(),
            auth_type: AuthType::Password,
            password: None,
            private_key_path: None,
            passphrase: None,
            remark: None,
        };
        assert!(validate_save_request(&req).is_err());
    }

    #[test]
    fn validate_rejects_blank_username() {
        let req = SaveHostRequest {
            id: "id3".to_string(),
            name: "prod".to_string(),
            host: "10.0.0.1".to_string(),
            port: 22,
            username: String::new(),
            auth_type: AuthType::Password,
            password: None,
            private_key_path: None,
            passphrase: None,
            remark: None,
        };
        assert!(validate_save_request(&req).is_err());
    }

    #[test]
    fn validate_accepts_valid_request() {
        let req = SaveHostRequest {
            id: "id4".to_string(),
            name: "prod".to_string(),
            host: "10.0.0.1".to_string(),
            port: 22,
            username: "root".to_string(),
            auth_type: AuthType::Password,
            password: None,
            private_key_path: None,
            passphrase: None,
            remark: None,
        };
        assert!(validate_save_request(&req).is_ok());
    }

    #[test]
    fn validate_rejects_empty_string_fields() {
        let req = SaveHostRequest {
            id: "id5".to_string(),
            name: String::new(),
            host: String::new(),
            port: 22,
            username: String::new(),
            auth_type: AuthType::Password,
            password: None,
            private_key_path: None,
            passphrase: None,
            remark: None,
        };
        assert!(validate_save_request(&req).is_err());
    }

    // --- 凭据引用回归测试（P0-5）---

    /// 模拟 save_host 中的凭据处理逻辑（不依赖 AppHandle）
    /// 验证：写入 key 与落盘引用值完全一致
    fn resolve_password_ref(
        request: &SaveHostRequest,
        existing: Option<&HostConfig>,
    ) -> Option<String> {
        match request.password.as_deref() {
            Some(pwd) if !pwd.is_empty() => Some(secure_store::password_key(&request.id)),
            _ => existing.and_then(|h| h.password_ref.clone()),
        }
    }

    /// 模拟 save_host 中的口令处理逻辑
    fn resolve_passphrase_ref(
        request: &SaveHostRequest,
        existing: Option<&HostConfig>,
    ) -> Option<String> {
        match request.passphrase.as_deref() {
            Some(pp) if !pp.is_empty() => Some(secure_store::passphrase_key(&request.id)),
            _ => existing.and_then(|h| h.passphrase_ref.clone()),
        }
    }

    #[test]
    fn new_host_with_password_generates_correct_ref() {
        // 新建主机时，非空密码应生成 titanssh:<id>:password 格式的引用
        let req = SaveHostRequest {
            id: "host-new".to_string(),
            name: "prod".to_string(),
            host: "10.0.0.1".to_string(),
            port: 22,
            username: "root".to_string(),
            auth_type: AuthType::Password,
            password: Some("secret123".to_string()),
            private_key_path: None,
            passphrase: None,
            remark: None,
        };
        let ref_val = resolve_password_ref(&req, None);
        assert_eq!(ref_val, Some("titanssh:host-new:password".to_string()));
    }

    #[test]
    fn edit_host_empty_password_preserves_old_ref() {
        // 编辑主机时，密码留空应保留旧引用（P0-2 修复验证）
        let existing = HostConfig {
            id: "host-1".to_string(),
            name: "prod".to_string(),
            host: "10.0.0.1".to_string(),
            port: 22,
            username: "root".to_string(),
            auth_type: AuthType::Password,
            password_ref: Some("titanssh:host-1:password".to_string()),
            private_key_path: None,
            passphrase_ref: None,
            remark: None,
        };
        let req = SaveHostRequest {
            id: "host-1".to_string(),
            name: "prod-updated".to_string(),
            host: "10.0.0.1".to_string(),
            port: 22,
            username: "root".to_string(),
            auth_type: AuthType::Password,
            // 密码留空，应保留旧引用
            password: Some(String::new()),
            private_key_path: None,
            passphrase: None,
            remark: None,
        };
        let ref_val = resolve_password_ref(&req, Some(&existing));
        assert_eq!(
            ref_val,
            Some("titanssh:host-1:password".to_string()),
            "编辑时密码留空应保留旧引用"
        );
    }

    #[test]
    fn edit_host_none_password_preserves_old_ref() {
        // 编辑主机时，password 字段为 None 也应保留旧引用
        let existing = HostConfig {
            id: "host-2".to_string(),
            name: "prod".to_string(),
            host: "10.0.0.2".to_string(),
            port: 22,
            username: "root".to_string(),
            auth_type: AuthType::Password,
            password_ref: Some("titanssh:host-2:password".to_string()),
            private_key_path: None,
            passphrase_ref: None,
            remark: None,
        };
        let req = SaveHostRequest {
            id: "host-2".to_string(),
            name: "prod".to_string(),
            host: "10.0.0.2".to_string(),
            port: 22,
            username: "root".to_string(),
            auth_type: AuthType::Password,
            password: None,
            private_key_path: None,
            passphrase: None,
            remark: None,
        };
        let ref_val = resolve_password_ref(&req, Some(&existing));
        assert_eq!(
            ref_val,
            Some("titanssh:host-2:password".to_string()),
            "password 为 None 时应保留旧引用"
        );
    }

    #[test]
    fn edit_host_empty_passphrase_preserves_old_ref() {
        // 编辑主机时，口令留空应保留旧引用（P0-2 修复验证）
        let existing = HostConfig {
            id: "host-3".to_string(),
            name: "prod".to_string(),
            host: "10.0.0.3".to_string(),
            port: 22,
            username: "root".to_string(),
            auth_type: AuthType::PrivateKey,
            password_ref: None,
            private_key_path: Some("~/.ssh/id_rsa".to_string()),
            passphrase_ref: Some("titanssh:host-3:passphrase".to_string()),
            remark: None,
        };
        let req = SaveHostRequest {
            id: "host-3".to_string(),
            name: "prod".to_string(),
            host: "10.0.0.3".to_string(),
            port: 22,
            username: "root".to_string(),
            auth_type: AuthType::PrivateKey,
            password: None,
            private_key_path: Some("~/.ssh/id_rsa".to_string()),
            // 口令留空，应保留旧引用
            passphrase: Some(String::new()),
            remark: None,
        };
        let ref_val = resolve_passphrase_ref(&req, Some(&existing));
        assert_eq!(
            ref_val,
            Some("titanssh:host-3:passphrase".to_string()),
            "编辑时口令留空应保留旧引用"
        );
    }

    #[test]
    fn new_host_no_password_has_no_ref() {
        // 新建主机且无密码时，password_ref 应为 None
        let req = SaveHostRequest {
            id: "host-new2".to_string(),
            name: "prod".to_string(),
            host: "10.0.0.1".to_string(),
            port: 22,
            username: "root".to_string(),
            auth_type: AuthType::Password,
            password: None,
            private_key_path: None,
            passphrase: None,
            remark: None,
        };
        let ref_val = resolve_password_ref(&req, None);
        assert_eq!(ref_val, None, "新建主机无密码时 password_ref 应为 None");
    }

    #[test]
    fn credential_key_ref_consistency_contract() {
        // 验证 password_key() 生成的 key 与落盘引用值格式完全一致
        // 即 save_host 写入 key == load_credentials 读取时使用的 ref 值
        let host_id = "host-contract-test";
        let key = secure_store::password_key(host_id);
        // 落盘引用值应等于写入 key（P0-1 修复的核心契约）
        assert_eq!(key, format!("titanssh:{}:password", host_id));
        // 确认 load_credentials 使用 password_ref 直接调用 get_credential 时能命中同一条目
        assert!(key.starts_with("titanssh:"), "key 必须以 titanssh: 开头");
    }
}