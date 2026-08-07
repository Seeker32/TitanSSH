use crate::errors::app_error::AppError;
use crate::models::host::HostConfig;
use std::fs;
use std::path::PathBuf;
use tauri::{AppHandle, Manager};

pub struct HostStore {
    file_path: PathBuf,
}

impl HostStore {
    /// 创建新的 HostStore 实例
    ///
    /// 通过 Tauri AppHandle 获取应用数据目录，确保目录存在后构建 hosts.json 文件路径。
    ///
    /// # 参数
    /// - `app_handle`: Tauri 应用句柄，用于解析平台相关的应用数据目录
    ///
    /// # 返回
    /// 成功返回 HostStore 实例，失败返回 StorageError
    pub fn new(app_handle: &AppHandle) -> Result<Self, AppError> {
        let app_data_dir = app_handle
            .path()
            .app_data_dir()
            .map_err(|error| AppError::StorageError(format!("无法获取应用数据目录: {error}")))?;

        // 确保数据目录存在，首次运行时自动创建
        fs::create_dir_all(&app_data_dir)
            .map_err(|error| AppError::StorageError(format!("无法创建应用数据目录: {error}")))?;

        let file_path = app_data_dir.join("hosts.json");

        Ok(Self { file_path })
    }

    /// 仅供测试使用：直接通过文件路径构造 HostStore，绕过 AppHandle
    #[cfg(test)]
    pub(crate) fn from_file_path(file_path: PathBuf) -> Self {
        Self { file_path }
    }

    /// 从持久化存储加载所有主机配置
    ///
    /// 若 hosts.json 不存在则返回空列表（首次运行场景）。
    /// 文件存在但内容非法时返回 StorageError。
    ///
    /// # 返回
    /// 成功返回主机配置列表，失败返回 StorageError
    pub fn load(&self) -> Result<Vec<HostConfig>, AppError> {
        // 文件不存在时返回空列表，对应首次运行场景
        if !self.file_path.exists() {
            return Ok(Vec::new());
        }

        let content = fs::read_to_string(&self.file_path)
            .map_err(|error| AppError::StorageError(format!("读取主机配置文件失败: {error}")))?;

        let hosts: Vec<HostConfig> = serde_json::from_str(&content)
            .map_err(|error| AppError::StorageError(format!("解析主机配置文件失败: {error}")))?;

        Ok(hosts)
    }

    /// 将主机配置列表持久化到 hosts.json
    ///
    /// 使用 pretty-print JSON 格式写入，便于人工排查问题。
    /// 写入前不含任何明文凭据，调用方必须确保已完成凭据剥离。
    ///
    /// # 参数
    /// - `hosts`: 要持久化的主机配置切片（不含明文凭据）
    pub fn save(&self, hosts: &[HostConfig]) -> Result<(), AppError> {
        let content = serde_json::to_string_pretty(hosts)
            .map_err(|error| AppError::StorageError(format!("序列化主机配置失败: {error}")))?;

        fs::write(&self.file_path, content)
            .map_err(|error| AppError::StorageError(format!("写入主机配置文件失败: {error}")))?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::HostStore;
    use crate::models::host::{AuthType, HostConfig};
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
        /// **验证: 需求 1.1, 1.5**
        ///
        /// Property 1: HostConfig 持久化往返
        ///
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
