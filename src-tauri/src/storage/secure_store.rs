use crate::errors::app_error::AppError;
use keyring::Entry;

/// 安全存储服务名，与 tauri.conf.json identifier 保持一致
const SERVICE_NAME: &str = "dev.titanssh.ssh-terminal-manager";

/// 将凭据写入 OS 安全存储（macOS Keychain / Windows Credential Manager / Linux Secret Service）
/// - key: 凭据的唯一标识键
/// - value: 要存储的明文凭据，存入后调用方应立即清除内存中的明文
pub fn set_credential(key: &str, value: &str) -> Result<(), AppError> {
    eprintln!("[secure_store] set_credential called with key: {}", key);
    let entry = Entry::new(SERVICE_NAME, key).map_err(|e| {
        eprintln!("[secure_store] Entry::new failed: {}", e);
        AppError::SecureStoreError(e.to_string())
    })?;
    eprintln!("[secure_store] Entry created, calling set_password...");
    entry.set_password(value).map_err(|e| {
        eprintln!("[secure_store] set_password failed: {}", e);
        AppError::SecureStoreError(e.to_string())
    })?;
    eprintln!("[secure_store] set_password succeeded");
    Ok(())
}

/// 从 OS 安全存储读取凭据
/// - key: 凭据的唯一标识键
/// - 返回明文凭据字符串，调用方使用完毕后应尽快释放
/// - 若凭据不存在，返回 CredentialNotFound 而非通用 SecureStoreError，便于上层给出明确提示
pub fn get_credential(key: &str) -> Result<String, AppError> {
    eprintln!("[secure_store] get_credential called with key: {}", key);
    let entry =
        Entry::new(SERVICE_NAME, key).map_err(|e| {
            eprintln!("[secure_store] Entry::new failed: {}", e);
            AppError::SecureStoreError(e.to_string())
        })?;
    eprintln!("[secure_store] Entry created, calling get_password...");
    entry.get_password().map_err(|e| {
        eprintln!("[secure_store] get_password failed with error: {:?}", e);
        // Check if it's a "not found" error by string matching
        let err_str = e.to_string();
        if err_str.contains("NoEntry") || err_str.contains("not found") || err_str.contains("不存在") {
            eprintln!("[secure_store] Error indicates credential not found");
            AppError::CredentialNotFound(key.to_string())
        } else {
            AppError::SecureStoreError(err_str)
        }
    })
}

/// 从 OS 安全存储删除凭据
/// - key: 凭据的唯一标识键
/// - 若凭据不存在则静默成功，避免删除时报错影响主流程
pub fn delete_credential(key: &str) -> Result<(), AppError> {
    let entry =
        Entry::new(SERVICE_NAME, key).map_err(|e| AppError::SecureStoreError(e.to_string()))?;
    match entry.delete_credential() {
        Ok(_) => Ok(()),
        // 凭据不存在时不视为错误
        Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(AppError::SecureStoreError(e.to_string())),
    }
}

/// 根据主机 ID 生成密码凭据的安全存储 key，格式为 titanssh-<id>-password
/// 此函数确保写入 key 与落盘引用值完全一致，消除 P0-1 不一致问题
pub fn password_key(host_id: &str) -> String {
    format!("titanssh-{}-password", host_id)
}

/// 根据主机 ID 生成私钥口令凭据的安全存储 key，格式为 titanssh-<id>-passphrase
pub fn passphrase_key(host_id: &str) -> String {
    format!("titanssh-{}-passphrase", host_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    /// 生成测试专用的唯一 key，避免与生产数据冲突
    fn test_key(suffix: &str) -> String {
        format!("titanssh-test-{}:{}", Uuid::new_v4(), suffix)
    }

    // --- key 格式契约测试（纯逻辑，不依赖 OS keyring）---

    #[test]
    fn password_key_format_matches_ref_format() {
        // 写入 key 必须与落盘引用值完全一致（修复 P0-1）
        let host_id = "host-abc123";
        let key = password_key(host_id);
        assert_eq!(key, "titanssh-host-abc123-password");
        // 确认 key 以 "titanssh-" 开头，与 load_credentials 使用的引用格式一致
        assert!(key.starts_with("titanssh-"));
    }

    #[test]
    fn passphrase_key_format_matches_ref_format() {
        let host_id = "host-xyz789";
        let key = passphrase_key(host_id);
        assert_eq!(key, "titanssh-host-xyz789-passphrase");
        assert!(key.starts_with("titanssh-"));
    }

    #[test]
    fn password_key_and_passphrase_key_are_distinct() {
        // 同一主机的密码 key 与口令 key 不得相同，避免覆盖
        let host_id = "host-1";
        assert_ne!(password_key(host_id), passphrase_key(host_id));
    }

    #[test]
    fn keys_for_different_hosts_are_distinct() {
        // 不同主机的 key 不得相同
        assert_ne!(password_key("host-1"), password_key("host-2"));
        assert_ne!(passphrase_key("host-1"), passphrase_key("host-2"));
    }

    // --- OS keyring 集成测试（set / get / delete 完整链路）---
    // 这些测试需要 OS 安全存储访问权限，在 CI 沙箱环境中跳过
    // 在真实机器上运行：cargo test -- --ignored

    #[test]
    #[ignore = "需要 OS keychain 访问权限，在 CI 环境中跳过"]
    fn set_get_delete_round_trip() {
        // 验证 set → get → delete 完整链路可正常工作
        let key = test_key("password");
        let value = "test-secret-value";

        set_credential(&key, value).expect("set_credential 应成功");
        let retrieved = get_credential(&key).expect("get_credential 应成功");
        assert_eq!(retrieved, value, "读取值应与写入值一致");

        delete_credential(&key).expect("delete_credential 应成功");
        // 删除后读取应失败
        assert!(
            get_credential(&key).is_err(),
            "删除后 get_credential 应返回错误"
        );
    }

    #[test]
    fn delete_nonexistent_credential_is_silent() {
        // 删除不存在的凭据应静默成功，不返回错误
        let key = test_key("nonexistent");
        let result = delete_credential(&key);
        assert!(
            result.is_ok(),
            "删除不存在凭据应静默成功，实际: {:?}",
            result
        );
    }

    #[test]
    #[ignore = "需要 OS keychain 访问权限，在 CI 环境中跳过"]
    fn overwrite_credential_returns_latest_value() {
        // 对同一 key 多次写入，get 应返回最新值
        let key = test_key("overwrite");

        set_credential(&key, "first-value").expect("第一次写入应成功");
        set_credential(&key, "second-value").expect("第二次写入应成功");

        let retrieved = get_credential(&key).expect("读取应成功");
        assert_eq!(retrieved, "second-value", "应返回最新写入的值");

        // 清理
        let _ = delete_credential(&key);
    }

    #[test]
    #[ignore = "需要 OS keychain 访问权限，在 CI 环境中跳过"]
    fn credential_key_ref_consistency() {
        // 验证 save_host 写入 key 与落盘引用值一致的完整链路：
        // 1. 使用 password_key() 生成 key 并写入
        // 2. 使用相同 key（即落盘引用值）读取
        // 3. 读取值与写入值一致
        let host_id = format!("test-host-{}", Uuid::new_v4());
        let key = password_key(&host_id);
        let secret = "my-ssh-password";

        set_credential(&key, secret).expect("写入应成功");
        // 模拟 load_credentials 使用落盘引用值（即 key 本身）读取
        let loaded = get_credential(&key).expect("使用引用值读取应成功");
        assert_eq!(loaded, secret, "引用值读取结果应与写入值一致");

        // 清理
        let _ = delete_credential(&key);
    }

    // --- 凭据链路契约测试（纯逻辑，不依赖 OS keyring）---

    /// 验证 password_key 与 passphrase_key 生成的 key 格式满足 load_credentials 的读取契约
    /// load_credentials 直接使用 password_ref / passphrase_ref 字段值调用 get_credential，
    /// 因此 save_host 写入时使用的 key 必须与落盘引用值完全一致
    #[test]
    fn password_key_matches_load_credentials_ref_format() {
        let host_id = "host-chain-test";
        let write_key = password_key(host_id);
        // 模拟 save_host 落盘的 password_ref 值
        let disk_ref = format!("titanssh-{}-password", host_id);
        assert_eq!(
            write_key, disk_ref,
            "写入 key 必须与落盘引用值完全一致，否则 load_credentials 无法读取"
        );
    }

    /// 验证 passphrase_key 与落盘引用值一致
    #[test]
    fn passphrase_key_matches_load_credentials_ref_format() {
        let host_id = "host-chain-test-2";
        let write_key = passphrase_key(host_id);
        let disk_ref = format!("titanssh-{}-passphrase", host_id);
        assert_eq!(write_key, disk_ref);
    }

    /// 验证 delete_credential 对不存在的 key 静默成功（NoEntry 不视为错误）
    #[test]
    fn delete_nonexistent_is_silent_no_entry() {
        // 使用唯一 key 确保不存在
        let key = test_key("silent-delete");
        let result = delete_credential(&key);
        assert!(
            result.is_ok(),
            "删除不存在的凭据应静默成功，实际: {:?}",
            result
        );
    }

    /// 验证 password_key 对不同 host_id 生成不同的 key（无碰撞）
    #[test]
    fn password_keys_for_different_hosts_have_no_collision() {
        let ids = ["host-a", "host-b", "host-c", "host-1", "host-2"];
        let keys: Vec<String> = ids.iter().map(|id| password_key(id)).collect();
        let unique: std::collections::HashSet<&String> = keys.iter().collect();
        assert_eq!(
            unique.len(),
            keys.len(),
            "不同 host_id 的 password_key 不得碰撞"
        );
    }

    /// 验证 passphrase_key 对不同 host_id 生成不同的 key（无碰撞）
    #[test]
    fn passphrase_keys_for_different_hosts_have_no_collision() {
        let ids = ["host-a", "host-b", "host-c"];
        let keys: Vec<String> = ids.iter().map(|id| passphrase_key(id)).collect();
        let unique: std::collections::HashSet<&String> = keys.iter().collect();
        assert_eq!(
            unique.len(),
            keys.len(),
            "不同 host_id 的 passphrase_key 不得碰撞"
        );
    }

    /// 验证 password_key 与 passphrase_key 对同一 host_id 生成不同的 key
    /// 防止密码与口令互相覆盖
    #[test]
    fn password_and_passphrase_keys_are_distinct_for_same_host() {
        let host_id = "host-same";
        assert_ne!(
            password_key(host_id),
            passphrase_key(host_id),
            "同一主机的密码 key 与口令 key 不得相同"
        );
    }

    /// 验证 key 格式不含明文凭据（key 本身不是密码）
    #[test]
    fn key_format_does_not_contain_plaintext() {
        let host_id = "host-plaintext-check";
        let fake_password = "super-secret-password-123";
        let key = password_key(host_id);
        assert!(!key.contains(fake_password), "key 不得包含明文凭据");
    }
}
