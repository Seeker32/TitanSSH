use crate::errors::app_error::AppError;
use keyring::Entry;

/// OS 安全存储服务名
const SERVICE_NAME: &str = "TitanSSH";

// --- Linux：Secret Service 主存储 + 内核 keyring（keyutils）回退 ---
//
// keyring 在 Linux 上默认使用 Secret Service（DBus，gnome-keyring / KWallet），
// 桌面环境未运行密钥环守护进程时写入直接失败。回退到内核 keyring 不依赖任何
// 守护进程；代价是凭据随重启丢失（kernel keyring 生命周期）。读取/删除需同时
// 覆盖两个存储，因为凭据可能落在任意一个。

#[cfg(target_os = "linux")]
mod linux {
    use super::*;

    /// 判定错误是否表示 Secret Service 守护不可用（无守护进程、无会话总线或
    /// 存储拒绝访问）；此类错误触发内核 keyring 回退，其余错误原样上抛。
    fn is_secret_service_unavailable(error: &keyring::Error) -> bool {
        matches!(
            error,
            keyring::Error::PlatformFailure(_) | keyring::Error::NoStorageAccess(_)
        )
    }

    fn secure_store_error(error: keyring::Error) -> AppError {
        AppError::SecureStoreError(error.to_string().into())
    }

    /// 构造 Secret Service 主条目（平台默认存储）
    pub(super) fn secret_service_entry(key: &str) -> Result<Entry, keyring::Error> {
        Entry::new(SERVICE_NAME, key)
    }

    /// 构造内核 keyutils 回退条目；无守护进程依赖，凭据随重启失效
    pub(super) fn keyutils_entry(key: &str) -> Result<Entry, keyring::Error> {
        let credential =
            keyring::keyutils::default_credential_builder().build(None, SERVICE_NAME, key)?;
        Ok(Entry::new_with_credential(credential))
    }

    /// 保存凭据：主存储（Secret Service）可用则写入；守护不可用时回退内核 keyring。
    ///
    /// 条目构造器参数使回退链可脱离真实 OS 存储测试（注入假条目）。
    pub(super) fn set_with_fallback(
        _key: &str,
        value: &str,
        primary: impl Fn() -> Result<Entry, keyring::Error>,
        fallback: impl Fn() -> Result<Entry, keyring::Error>,
    ) -> Result<(), AppError> {
        let primary_entry = match primary() {
            Ok(entry) => entry,
            Err(error) if is_secret_service_unavailable(&error) => {
                let entry = fallback().map_err(secure_store_error)?;
                return entry.set_password(value).map_err(secure_store_error);
            }
            Err(error) => return Err(secure_store_error(error)),
        };
        match primary_entry.set_password(value) {
            Ok(()) => Ok(()),
            Err(error) if is_secret_service_unavailable(&error) => {
                let entry = fallback().map_err(secure_store_error)?;
                entry.set_password(value).map_err(secure_store_error)
            }
            Err(error) => Err(secure_store_error(error)),
        }
    }

    /// 读取凭据：主存储优先；无记录或守护不可用时读取回退存储。
    pub(super) fn get_with_fallback(
        key: &str,
        primary: impl Fn() -> Result<Entry, keyring::Error>,
        fallback: impl Fn() -> Result<Entry, keyring::Error>,
    ) -> Result<String, AppError> {
        let primary_entry = match primary() {
            Ok(entry) => entry,
            Err(error) if is_secret_service_unavailable(&error) => {
                return read_fallback(key, fallback);
            }
            Err(error) => return Err(secure_store_error(error)),
        };
        match primary_entry.get_password() {
            Ok(value) => Ok(value),
            Err(keyring::Error::NoEntry) => read_fallback(key, fallback),
            Err(error) if is_secret_service_unavailable(&error) => read_fallback(key, fallback),
            Err(error) => Err(secure_store_error(error)),
        }
    }

    /// 仅从回退存储读取；无记录时映射为 CredentialNotFound
    fn read_fallback(
        key: &str,
        fallback: impl Fn() -> Result<Entry, keyring::Error>,
    ) -> Result<String, AppError> {
        match fallback().map_err(secure_store_error)?.get_password() {
            Ok(value) => Ok(value),
            Err(keyring::Error::NoEntry) => {
                Err(AppError::CredentialNotFound(key.to_string().into()))
            }
            Err(error) => Err(secure_store_error(error)),
        }
    }

    /// 从两个存储删除凭据（幂等）：凭据可能落在任意一个存储。
    /// 无记录与守护不可用不视为错误；任一存储报硬错误时整体失败，但另一存储仍被清理。
    pub(super) fn delete_with_fallback(
        primary: impl Fn() -> Result<Entry, keyring::Error>,
        fallback: impl Fn() -> Result<Entry, keyring::Error>,
    ) -> Result<(), AppError> {
        let mut first_error = None;
        attempt_delete(primary(), &mut first_error);
        attempt_delete(fallback(), &mut first_error);
        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    /// 单存储删除尝试；忽略 NoEntry 与守护不可用，硬错误只记录第一个
    fn attempt_delete(
        entry_result: Result<Entry, keyring::Error>,
        first_error: &mut Option<AppError>,
    ) {
        match entry_result {
            Ok(entry) => match entry.delete_credential() {
                Ok(()) => {}
                Err(keyring::Error::NoEntry) => {}
                Err(error) if is_secret_service_unavailable(&error) => {}
                Err(error) => {
                    if first_error.is_none() {
                        *first_error = Some(secure_store_error(error));
                    }
                }
            },
            Err(error) if is_secret_service_unavailable(&error) => {}
            Err(error) => {
                if first_error.is_none() {
                    *first_error = Some(secure_store_error(error));
                }
            }
        }
    }
}

/// 将凭据写入 OS 安全存储（macOS Keychain / Windows Credential Manager / Linux Secret Service，
/// Linux 无密钥环守护时回退内核 keyring）
/// - key: 凭据的唯一标识键
/// - value: 要存储的明文凭据，存入后调用方应立即清除内存中的明文
#[cfg(not(target_os = "linux"))]
pub fn set_credential(key: &str, value: &str) -> Result<(), AppError> {
    let entry = Entry::new(SERVICE_NAME, key)
        .map_err(|e| AppError::SecureStoreError(e.to_string().into()))?;
    entry
        .set_password(value)
        .map_err(|e| AppError::SecureStoreError(e.to_string().into()))
}

/// Linux 版本：Secret Service 主存储，守护不可用时回退内核 keyring
#[cfg(target_os = "linux")]
pub fn set_credential(key: &str, value: &str) -> Result<(), AppError> {
    linux::set_with_fallback(
        key,
        value,
        || linux::secret_service_entry(key),
        || linux::keyutils_entry(key),
    )
}

/// 从 OS 安全存储读取凭据
/// - key: 凭据的唯一标识键
/// - 返回明文凭据字符串，调用方使用完毕后应尽快释放
/// - 若凭据不存在，返回 CredentialNotFound 而非通用 SecureStoreError，便于上层给出明确提示
#[cfg(not(target_os = "linux"))]
pub fn get_credential(key: &str) -> Result<String, AppError> {
    let entry = Entry::new(SERVICE_NAME, key)
        .map_err(|e| AppError::SecureStoreError(e.to_string().into()))?;
    entry.get_password().map_err(|error| {
        if matches!(error, keyring::Error::NoEntry) {
            AppError::CredentialNotFound(key.to_string().into())
        } else {
            AppError::SecureStoreError(error.to_string().into())
        }
    })
}

/// Linux 版本：主存储优先，无记录或守护不可用时读取回退存储
#[cfg(target_os = "linux")]
pub fn get_credential(key: &str) -> Result<String, AppError> {
    linux::get_with_fallback(
        key,
        || linux::secret_service_entry(key),
        || linux::keyutils_entry(key),
    )
}

/// 从 OS 安全存储删除凭据；macOS 直接按属性删除，避免 keyring 先读取密码触发授权弹窗
/// - key: 凭据的唯一标识键
/// - 若凭据不存在则静默成功，避免删除时报错影响主流程
#[cfg(target_os = "macos")]
pub fn delete_credential(key: &str) -> Result<(), AppError> {
    delete_macos_credential_with(key, delete_macos_item)
}

/// 执行 macOS Keychain 直接删除查询；回调参数依次为 service 与 account
#[cfg(target_os = "macos")]
fn delete_macos_credential_with(
    key: &str,
    delete: impl FnOnce(&str, &str) -> security_framework::base::Result<()>,
) -> Result<(), AppError> {
    const ERR_SEC_ITEM_NOT_FOUND: i32 = -25300;

    match delete(SERVICE_NAME, key) {
        Ok(()) => Ok(()),
        Err(error) if error.code() == ERR_SEC_ITEM_NOT_FOUND => Ok(()),
        Err(error) => Err(AppError::SecureStoreError(error.to_string().into())),
    }
}

/// 从用户默认 Keychain 直接删除匹配项，不读取其中的密码数据
#[cfg(target_os = "macos")]
fn delete_macos_item(service: &str, account: &str) -> security_framework::base::Result<()> {
    use security_framework::item::{ItemClass, ItemSearchOptions};
    use security_framework::os::macos::keychain::{SecKeychain, SecPreferencesDomain};

    let keychain = SecKeychain::default_for_domain(SecPreferencesDomain::User)?;
    let mut query = ItemSearchOptions::new();
    query
        .keychains(&[keychain])
        .class(ItemClass::generic_password())
        .service(service)
        .account(account);
    query.delete()
}

/// 从非 macOS、非 Linux 的 OS 安全存储删除凭据；不存在时静默成功
#[cfg(all(not(target_os = "macos"), not(target_os = "linux")))]
pub fn delete_credential(key: &str) -> Result<(), AppError> {
    let entry = Entry::new(SERVICE_NAME, key)
        .map_err(|e| AppError::SecureStoreError(e.to_string().into()))?;
    match entry.delete_credential() {
        Ok(_) => Ok(()),
        // 凭据不存在时不视为错误
        Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(AppError::SecureStoreError(e.to_string().into())),
    }
}

/// Linux 版本：同时清理主存储与回退存储（凭据可能落在任意一个）
#[cfg(target_os = "linux")]
pub fn delete_credential(key: &str) -> Result<(), AppError> {
    linux::delete_with_fallback(
        || linux::secret_service_entry(key),
        || linux::keyutils_entry(key),
    )
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

// --- Linux 双存储回退链单元测试（注入假凭据条目，不依赖 OS 安全存储）---

#[cfg(all(test, target_os = "linux"))]
mod fallback_tests {
    use super::linux::{delete_with_fallback, get_with_fallback, set_with_fallback};
    use super::*;
    use keyring::credential::CredentialApi;
    use std::sync::{Arc, Mutex};

    /// 可注入一次性失败的假凭据条目：所有调用共享同一内部状态
    #[derive(Clone, Default)]
    struct FakeCredential {
        state: Arc<Mutex<FakeCredentialState>>,
    }

    #[derive(Default)]
    struct FakeCredentialState {
        password: Option<String>,
        /// 下一次 set 返回的错误（一次性）
        fail_set: Option<keyring::Error>,
    }

    impl FakeCredential {
        fn failing_set(error: keyring::Error) -> Self {
            Self {
                state: Arc::new(Mutex::new(FakeCredentialState {
                    fail_set: Some(error),
                    ..FakeCredentialState::default()
                })),
            }
        }

        fn password(&self) -> Option<String> {
            self.state
                .lock()
                .expect("state lock poisoned")
                .password
                .clone()
        }
    }

    impl CredentialApi for FakeCredential {
        fn set_secret(&self, secret: &[u8]) -> keyring::Result<()> {
            let mut state = self.state.lock().expect("state lock poisoned");
            if let Some(error) = state.fail_set.take() {
                return Err(error);
            }
            state.password = Some(String::from_utf8_lossy(secret).into_owned());
            Ok(())
        }

        fn get_secret(&self) -> keyring::Result<Vec<u8>> {
            let state = self.state.lock().expect("state lock poisoned");
            match &state.password {
                Some(value) => Ok(value.clone().into_bytes()),
                None => Err(keyring::Error::NoEntry),
            }
        }

        fn delete_credential(&self) -> keyring::Result<()> {
            let mut state = self.state.lock().expect("state lock poisoned");
            match state.password.take() {
                Some(_) => Ok(()),
                None => Err(keyring::Error::NoEntry),
            }
        }

        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
    }

    /// 把假凭据包装成条目构造器，每次调用构建共享同一状态的新条目
    fn entry(fake: &FakeCredential) -> impl Fn() -> Result<keyring::Entry, keyring::Error> + '_ {
        let fake = fake.clone();
        move || Ok(keyring::Entry::new_with_credential(Box::new(fake.clone())))
    }

    /// 模拟 Secret Service 守护缺失（DBus 服务未注册）
    fn platform_failure() -> keyring::Error {
        keyring::Error::PlatformFailure(Box::new(std::io::Error::other(
            "The name org.freedesktop.secrets was not provided by any .service files",
        )))
    }

    // --- set 回退链 ---

    /// 主存储写入报守护不可用：回退到内核 keyring，保存成功
    #[test]
    fn set_falls_back_when_primary_store_unavailable() {
        let primary = FakeCredential::failing_set(platform_failure());
        let fallback = FakeCredential::default();

        set_with_fallback("key", "secret", entry(&primary), entry(&fallback))
            .expect("回退保存应成功");

        assert_eq!(fallback.password(), Some("secret".to_string()));
        assert_eq!(primary.password(), None, "主存储不应被写入");
    }

    /// 主存储无访问权限同样触发回退
    #[test]
    fn set_falls_back_when_primary_store_denies_access() {
        let primary = FakeCredential::failing_set(keyring::Error::NoStorageAccess(Box::new(
            std::io::Error::other("keyring locked"),
        )));
        let fallback = FakeCredential::default();

        set_with_fallback("key", "secret", entry(&primary), entry(&fallback))
            .expect("回退保存应成功");

        assert_eq!(fallback.password(), Some("secret".to_string()));
    }

    /// 主存储条目构造失败（无会话总线）：同样回退
    #[test]
    fn set_falls_back_when_primary_entry_build_fails() {
        let fallback = FakeCredential::default();

        set_with_fallback(
            "key",
            "secret",
            || Err(platform_failure()),
            entry(&fallback),
        )
        .expect("回退保存应成功");

        assert_eq!(fallback.password(), Some("secret".to_string()));
    }

    /// 主存储正常：直接写入主存储，不触碰回退存储
    #[test]
    fn set_uses_primary_when_available() {
        let primary = FakeCredential::default();
        let fallback = FakeCredential::default();

        set_with_fallback("key", "secret", entry(&primary), entry(&fallback))
            .expect("主存储保存应成功");

        assert_eq!(primary.password(), Some("secret".to_string()));
        assert_eq!(fallback.password(), None);
    }

    /// 主存储报非可用性错误：原样上抛，不尝试回退
    #[test]
    fn set_propagates_primary_hard_error_without_fallback() {
        let primary = FakeCredential::failing_set(keyring::Error::NoEntry);
        let fallback = FakeCredential::default();

        let error = set_with_fallback("key", "secret", entry(&primary), entry(&fallback))
            .expect_err("非可用性错误应上抛");

        assert!(matches!(error, AppError::SecureStoreError(_)));
        assert_eq!(fallback.password(), None, "回退存储不应被写入");
    }

    /// 主存储不可用且回退存储构造失败：报安全存储错误
    #[test]
    fn set_reports_error_when_both_stores_unavailable() {
        let error = set_with_fallback(
            "key",
            "secret",
            || Err(platform_failure()),
            || {
                Err(keyring::Error::PlatformFailure(Box::new(
                    std::io::Error::other("keyrings disabled"),
                )))
            },
        )
        .expect_err("双存储均不可用应报错");

        assert!(matches!(error, AppError::SecureStoreError(_)));
    }

    // --- get 回退链 ---

    /// 主存储有值：直接返回主存储凭据
    #[test]
    fn get_returns_primary_value() {
        let primary = FakeCredential::default();
        let fallback = FakeCredential::default();
        set_with_fallback("key", "primary-secret", entry(&primary), entry(&fallback))
            .expect("set 应成功");

        let value =
            get_with_fallback("key", entry(&primary), entry(&fallback)).expect("get 应成功");
        assert_eq!(value, "primary-secret");
    }

    /// 主存储无记录（此前经回退保存）：返回回退存储凭据
    #[test]
    fn get_falls_back_when_primary_has_no_entry() {
        let primary = FakeCredential::default();
        let fallback = FakeCredential::default();
        set_with_fallback(
            "key",
            "fallback-secret",
            || Err(platform_failure()),
            entry(&fallback),
        )
        .expect("回退 set 应成功");

        let value =
            get_with_fallback("key", entry(&primary), entry(&fallback)).expect("get 应成功");
        assert_eq!(value, "fallback-secret");
    }

    /// 主存储守护不可用：读取回退存储
    #[test]
    fn get_falls_back_when_primary_unavailable() {
        let fallback = FakeCredential::default();
        set_with_fallback(
            "key",
            "fallback-secret",
            || Err(platform_failure()),
            entry(&fallback),
        )
        .expect("回退 set 应成功");

        let value = get_with_fallback("key", || Err(platform_failure()), entry(&fallback))
            .expect("get 应回退成功");
        assert_eq!(value, "fallback-secret");
    }

    /// 双存储均无记录：返回 CredentialNotFound（携带 key）
    #[test]
    fn get_returns_not_found_when_both_stores_empty() {
        let primary = FakeCredential::default();
        let fallback = FakeCredential::default();

        let error = get_with_fallback("key", entry(&primary), entry(&fallback))
            .expect_err("双存储均无记录应返回 CredentialNotFound");

        assert!(matches!(error, AppError::CredentialNotFound(ref key) if key.to_string() == "key"));
    }

    // --- delete 回退链 ---

    /// 双存储各有记录：全部清理
    #[test]
    fn delete_clears_both_stores() {
        let primary = FakeCredential::default();
        let fallback = FakeCredential::default();
        set_with_fallback("key", "a", entry(&primary), entry(&fallback)).expect("set 应成功");
        set_with_fallback("key", "b", || Err(platform_failure()), entry(&fallback))
            .expect("set 应成功");

        delete_with_fallback(entry(&primary), entry(&fallback)).expect("delete 应成功");

        assert_eq!(primary.password(), None);
        assert_eq!(fallback.password(), None);
    }

    /// 单存储有记录：幂等成功
    #[test]
    fn delete_is_idempotent_when_stores_empty() {
        let primary = FakeCredential::default();
        let fallback = FakeCredential::default();

        delete_with_fallback(entry(&primary), entry(&fallback)).expect("空存储删除应成功");
    }

    /// 主存储不可用：仅清理回退存储，整体成功
    #[test]
    fn delete_ignores_unavailable_primary() {
        let fallback = FakeCredential::default();
        set_with_fallback(
            "key",
            "secret",
            || Err(platform_failure()),
            entry(&fallback),
        )
        .expect("set 应成功");

        delete_with_fallback(|| Err(platform_failure()), entry(&fallback)).expect("delete 应成功");
        assert_eq!(fallback.password(), None);
    }

    /// 任一存储报硬错误：整体失败且另一个存储仍被清理
    #[test]
    fn delete_reports_hard_error_and_still_cleans_other_store() {
        // 主存储条目构造即报硬错误，回退存储正常持有记录
        let fallback = FakeCredential::default();
        set_with_fallback(
            "key",
            "secret",
            || Err(platform_failure()),
            entry(&fallback),
        )
        .expect("set 应成功");

        let error = delete_with_fallback(|| Err(keyring::Error::NoEntry), entry(&fallback));

        assert!(error.is_err());
        assert_eq!(fallback.password(), None, "另一存储仍应被清理");
    }
}

#[cfg(test)]
#[path = "secure_store_test.rs"]
mod tests;
