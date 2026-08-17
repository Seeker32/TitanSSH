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

    /// 仅判定 DBus 明确报告 Secret Service 未注册或没有拥有者；锁定集合、拒绝访问
    /// 及其他平台错误均须原样上抛，避免把持久凭据悄然降级到易失内核 keyring。
    fn is_secret_service_unavailable(error: &keyring::Error) -> bool {
        const DAEMON_ABSENT_ERROR_NAMES: [&str; 2] = [
            "org.freedesktop.DBus.Error.ServiceUnknown",
            "org.freedesktop.DBus.Error.NameHasNoOwner",
        ];

        matches!(
            error,
            keyring::Error::PlatformFailure(platform_error)
                if matches!(
                    platform_error.downcast_ref::<dbus_secret_service::Error>(),
                    Some(dbus_secret_service::Error::Dbus(dbus_error))
                        if dbus_error
                            .name()
                            .is_some_and(|name| DAEMON_ABSENT_ERROR_NAMES.contains(&name))
                )
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
                entry.set_password(value).map_err(secure_store_error)?;
                attempt_cleanup(primary());
                return Ok(());
            }
            Err(error) => return Err(secure_store_error(error)),
        };
        match primary_entry.set_password(value) {
            Ok(()) => {
                attempt_cleanup(fallback());
                Ok(())
            }
            Err(error) if is_secret_service_unavailable(&error) => {
                let entry = fallback().map_err(secure_store_error)?;
                entry.set_password(value).map_err(secure_store_error)?;
                attempt_cleanup(primary());
                Ok(())
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
    /// 无记录不视为错误；仅主存储守护未注册可忽略，其他错误整体失败，但另一存储仍被清理。
    pub(super) fn delete_with_fallback(
        primary: impl Fn() -> Result<Entry, keyring::Error>,
        fallback: impl Fn() -> Result<Entry, keyring::Error>,
    ) -> Result<(), AppError> {
        let mut first_error = None;
        attempt_delete(primary(), true, &mut first_error);
        attempt_delete(fallback(), false, &mut first_error);
        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    /// 单存储删除尝试；仅主存储的守护未注册错误可忽略，硬错误只记录第一个
    fn attempt_delete(
        entry_result: Result<Entry, keyring::Error>,
        is_primary: bool,
        first_error: &mut Option<AppError>,
    ) {
        match entry_result {
            Ok(entry) => match entry.delete_credential() {
                Ok(()) => {}
                Err(keyring::Error::NoEntry) => {}
                Err(error) if is_primary && is_secret_service_unavailable(&error) => {}
                Err(error) => {
                    if first_error.is_none() {
                        *first_error = Some(secure_store_error(error));
                    }
                }
            },
            Err(error) if is_primary && is_secret_service_unavailable(&error) => {}
            Err(error) => {
                if first_error.is_none() {
                    *first_error = Some(secure_store_error(error));
                }
            }
        }
    }

    /// 尝试清理写入目标的另一存储；所有错误均忽略，避免清理失败掩盖已成功的凭据写入
    fn attempt_cleanup(entry_result: Result<Entry, keyring::Error>) {
        let mut ignored_error = None;
        attempt_delete(entry_result, false, &mut ignored_error);
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

/// macOS Keychain 删除范围；必须与 keyring apple-native 的读取范围保持一致。
#[cfg(target_os = "macos")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MacosKeychainScope {
    /// 不指定 kSecMatchSearchList，让 Security Framework 查询用户完整搜索列表。
    UserSearchList,
}

/// 执行 macOS Keychain 直接删除查询；回调参数依次为 service、account 与搜索范围。
#[cfg(target_os = "macos")]
fn delete_macos_credential_with(
    key: &str,
    delete: impl FnOnce(&str, &str, MacosKeychainScope) -> security_framework::base::Result<()>,
) -> Result<(), AppError> {
    const ERR_SEC_ITEM_NOT_FOUND: i32 = -25300;

    match delete(SERVICE_NAME, key, MacosKeychainScope::UserSearchList) {
        Ok(()) => Ok(()),
        Err(error) if error.code() == ERR_SEC_ITEM_NOT_FOUND => Ok(()),
        Err(error) => Err(AppError::SecureStoreError(error.to_string().into())),
    }
}

/// 从用户完整 Keychain 搜索列表直接删除匹配项，不读取其中的密码数据。
///
/// 不得设置 ItemSearchOptions::keychains：该字段会把查询限制为给定钥匙串；省略后
/// SecItemDelete 使用系统搜索列表，与 keyring apple-native 的 SecItemCopyMatching 查询一致。
#[cfg(target_os = "macos")]
fn delete_macos_item(
    service: &str,
    account: &str,
    scope: MacosKeychainScope,
) -> security_framework::base::Result<()> {
    use security_framework::item::{ItemClass, ItemSearchOptions};

    let mut query = ItemSearchOptions::new();
    match scope {
        MacosKeychainScope::UserSearchList => {
            query
                .class(ItemClass::generic_password())
                .service(service)
                .account(account);
        }
    }
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
        /// 下一次 get 返回的错误（一次性）
        fail_get: Option<keyring::Error>,
        /// 下一次 delete 返回的错误（一次性）
        fail_delete: Option<keyring::Error>,
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

        fn with_password_and_failing_set(password: &str, error: keyring::Error) -> Self {
            Self {
                state: Arc::new(Mutex::new(FakeCredentialState {
                    password: Some(password.to_string()),
                    fail_set: Some(error),
                    ..FakeCredentialState::default()
                })),
            }
        }

        fn with_password_and_failing_delete(password: &str, error: keyring::Error) -> Self {
            Self {
                state: Arc::new(Mutex::new(FakeCredentialState {
                    password: Some(password.to_string()),
                    fail_delete: Some(error),
                    ..FakeCredentialState::default()
                })),
            }
        }

        fn failing_get(error: keyring::Error) -> Self {
            Self {
                state: Arc::new(Mutex::new(FakeCredentialState {
                    fail_get: Some(error),
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
            let mut state = self.state.lock().expect("state lock poisoned");
            if let Some(error) = state.fail_get.take() {
                return Err(error);
            }
            match &state.password {
                Some(value) => Ok(value.clone().into_bytes()),
                None => Err(keyring::Error::NoEntry),
            }
        }

        fn delete_credential(&self) -> keyring::Result<()> {
            let mut state = self.state.lock().expect("state lock poisoned");
            if let Some(error) = state.fail_delete.take() {
                return Err(error);
            }
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

    /// 构造 Secret Service 守护未注册时返回的具名 DBus 错误
    fn daemon_absent_error(error_name: &str) -> keyring::Error {
        let dbus_error = dbus::Error::new_custom(
            error_name,
            "The name org.freedesktop.secrets was not provided by any .service files",
        );
        keyring::Error::PlatformFailure(Box::new(dbus_secret_service::Error::Dbus(dbus_error)))
    }

    /// 模拟 DBus 无法激活 Secret Service 服务
    fn service_unknown() -> keyring::Error {
        daemon_absent_error("org.freedesktop.DBus.Error.ServiceUnknown")
    }

    /// 模拟 Secret Service 名称当前没有拥有者
    fn name_has_no_owner() -> keyring::Error {
        daemon_absent_error("org.freedesktop.DBus.Error.NameHasNoOwner")
    }

    // --- set 回退链 ---

    /// 主存储写入报守护不可用：回退到内核 keyring，保存成功
    #[test]
    fn set_falls_back_when_primary_store_unavailable() {
        let primary = FakeCredential::failing_set(service_unknown());
        let fallback = FakeCredential::default();

        set_with_fallback("key", "secret", entry(&primary), entry(&fallback))
            .expect("回退保存应成功");

        assert_eq!(fallback.password(), Some("secret".to_string()));
        assert_eq!(primary.password(), None, "主存储不应被写入");
    }

    /// 主存储被锁定或拒绝访问：向调用方报告错误，不写入易失回退存储
    #[test]
    fn set_propagates_primary_access_error_without_fallback() {
        let primary = FakeCredential::failing_set(keyring::Error::NoStorageAccess(Box::new(
            std::io::Error::other("keyring locked"),
        )));
        let fallback = FakeCredential::default();

        let error = set_with_fallback("key", "secret", entry(&primary), entry(&fallback))
            .expect_err("主存储被锁定时应报告安全存储错误");

        assert!(matches!(error, AppError::SecureStoreError(_)));
        assert_eq!(fallback.password(), None, "回退存储不应被写入");
    }

    /// 主存储条目构造报告守护未注册：同样回退
    #[test]
    fn set_falls_back_when_primary_entry_build_fails() {
        let fallback = FakeCredential::default();

        set_with_fallback("key", "secret", || Err(service_unknown()), entry(&fallback))
            .expect("回退保存应成功");

        assert_eq!(fallback.password(), Some("secret".to_string()));
    }

    /// 主存储名称没有拥有者时：回退到内核 keyring，保存成功
    #[test]
    fn set_falls_back_when_primary_store_name_has_no_owner() {
        let primary = FakeCredential::failing_set(name_has_no_owner());
        let fallback = FakeCredential::default();

        set_with_fallback("key", "secret", entry(&primary), entry(&fallback))
            .expect("回退保存应成功");

        assert_eq!(fallback.password(), Some("secret".to_string()));
    }

    /// 主存储正常写入新凭据后：清除回退存储中的旧凭据，避免守护不可用时读取过期值
    #[test]
    fn set_clears_stale_fallback_after_primary_write() {
        let primary = FakeCredential::default();
        let fallback = FakeCredential::default();
        fallback
            .set_secret(b"stale-fallback-secret")
            .expect("预置回退存储凭据应成功");

        set_with_fallback("key", "primary-secret", entry(&primary), entry(&fallback))
            .expect("主存储保存应成功");

        assert_eq!(primary.password(), Some("primary-secret".to_string()));
        assert_eq!(fallback.password(), None, "回退存储旧凭据应被清理");
    }

    /// 回退存储正常写入新凭据后：清除主存储中的旧凭据，避免守护恢复前后读取过期值
    #[test]
    fn set_clears_stale_primary_after_fallback_write() {
        let primary = FakeCredential::with_password_and_failing_set(
            "stale-primary-secret",
            service_unknown(),
        );
        let fallback = FakeCredential::default();

        set_with_fallback("key", "fallback-secret", entry(&primary), entry(&fallback))
            .expect("回退存储保存应成功");

        assert_eq!(fallback.password(), Some("fallback-secret".to_string()));
        assert_eq!(primary.password(), None, "主存储旧凭据应被清理");
    }

    /// 清理另一存储失败不影响已成功写入的凭据
    #[test]
    fn set_succeeds_when_opposite_store_cleanup_fails() {
        let primary = FakeCredential::default();
        let fallback = FakeCredential::with_password_and_failing_delete(
            "stale-fallback-secret",
            service_unknown(),
        );

        set_with_fallback("key", "primary-secret", entry(&primary), entry(&fallback))
            .expect("主存储保存成功不应被回退存储清理失败掩盖");

        assert_eq!(primary.password(), Some("primary-secret".to_string()));
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
            || Err(service_unknown()),
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
            || Err(service_unknown()),
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
            || Err(service_unknown()),
            entry(&fallback),
        )
        .expect("回退 set 应成功");

        let value = get_with_fallback("key", || Err(service_unknown()), entry(&fallback))
            .expect("get 应回退成功");
        assert_eq!(value, "fallback-secret");
    }

    /// 主存储被锁定或拒绝访问：向调用方报告错误，不伪装为凭据不存在
    #[test]
    fn get_propagates_primary_access_error_without_fallback() {
        let primary = FakeCredential::failing_get(keyring::Error::NoStorageAccess(Box::new(
            std::io::Error::other("keyring locked"),
        )));
        let fallback = FakeCredential::default();

        let error = get_with_fallback("key", entry(&primary), entry(&fallback))
            .expect_err("主存储被锁定时应报告安全存储错误");

        assert!(matches!(error, AppError::SecureStoreError(_)));
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
        set_with_fallback("key", "b", || Err(service_unknown()), entry(&fallback))
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
        set_with_fallback("key", "secret", || Err(service_unknown()), entry(&fallback))
            .expect("set 应成功");

        delete_with_fallback(|| Err(service_unknown()), entry(&fallback)).expect("delete 应成功");
        assert_eq!(fallback.password(), None);
    }

    /// 主存储被锁定或拒绝访问：向调用方报告错误，但仍清理另一存储
    #[test]
    fn delete_propagates_primary_access_error_and_cleans_fallback() {
        let primary = FakeCredential::with_password_and_failing_delete(
            "primary-secret",
            keyring::Error::NoStorageAccess(Box::new(std::io::Error::other("keyring locked"))),
        );
        let fallback = FakeCredential::default();
        fallback
            .set_secret(b"fallback-secret")
            .expect("预置回退存储凭据应成功");

        let error = delete_with_fallback(entry(&primary), entry(&fallback))
            .expect_err("主存储被锁定时应报告安全存储错误");

        assert!(matches!(error, AppError::SecureStoreError(_)));
        assert_eq!(fallback.password(), None, "另一存储仍应被清理");
    }

    /// 回退内核 keyring 删除失败：向调用方报告错误，不能伪装为删除成功
    #[test]
    fn delete_propagates_fallback_platform_error() {
        let primary = FakeCredential::default();
        let fallback = FakeCredential::with_password_and_failing_delete(
            "fallback-secret",
            keyring::Error::PlatformFailure(Box::new(std::io::Error::from_raw_os_error(13))),
        );

        let error = delete_with_fallback(entry(&primary), entry(&fallback))
            .expect_err("回退内核 keyring 删除失败时应报告安全存储错误");

        assert!(matches!(error, AppError::SecureStoreError(_)));
        assert_eq!(
            fallback.password(),
            Some("fallback-secret".to_string()),
            "删除失败的回退凭据应保持可见"
        );
    }

    /// 任一存储报硬错误：整体失败且另一个存储仍被清理
    #[test]
    fn delete_reports_hard_error_and_still_cleans_other_store() {
        // 主存储条目构造即报硬错误，回退存储正常持有记录
        let fallback = FakeCredential::default();
        set_with_fallback("key", "secret", || Err(service_unknown()), entry(&fallback))
            .expect("set 应成功");

        let error = delete_with_fallback(|| Err(keyring::Error::NoEntry), entry(&fallback));

        assert!(error.is_err());
        assert_eq!(fallback.password(), None, "另一存储仍应被清理");
    }
}

#[cfg(test)]
#[path = "secure_store_test.rs"]
mod tests;
