use crate::errors::app_error::AppError;
use crate::models::host::HostConfig;
use std::fs;
use std::path::PathBuf;
use tauri::{AppHandle, Manager};

pub struct HostStore {
    file_path: PathBuf,
}

impl HostStore {
    /// Creates a new HostStore instance
    /// 
    /// # Arguments
    /// * `app_handle` - Tauri application handle for accessing app data directory
    /// 
    /// # Returns
    /// * `Result<Self, AppError>` - New HostStore instance or error
    pub fn new(app_handle: &AppHandle) -> Result<Self, AppError> {
        let app_data_dir = app_handle.path().app_data_dir().map_err(|error| {
            AppError::StorageError(format!("Failed to get app data dir: {error}"))
        })?;

        // Create directory if it doesn't exist
        fs::create_dir_all(&app_data_dir).map_err(|error| {
            AppError::StorageError(format!("Failed to create app data directory: {error}"))
        })?;

        let file_path = app_data_dir.join("hosts.json");

        Ok(Self { file_path })
    }

    #[cfg(test)]
    fn from_file_path(file_path: PathBuf) -> Self {
        Self { file_path }
    }

    /// Loads all host configurations from storage
    /// 
    /// # Returns
    /// * `Result<Vec<HostConfig>, AppError>` - List of host configs or error
    pub fn load(&self) -> Result<Vec<HostConfig>, AppError> {
        // Return empty list if file doesn't exist
        if !self.file_path.exists() {
            return Ok(Vec::new());
        }

        let content = fs::read_to_string(&self.file_path).map_err(|error| {
            AppError::StorageError(format!("Failed to read hosts file: {error}"))
        })?;

        let hosts: Vec<HostConfig> = serde_json::from_str(&content).map_err(|error| {
            AppError::StorageError(format!("Failed to parse hosts file: {error}"))
        })?;

        Ok(hosts)
    }

    /// Saves host configurations to storage
    /// 
    /// # Arguments
    /// * `hosts` - Slice of host configurations to save
    /// 
    /// # Returns
    /// * `Result<(), AppError>` - Success or error
    pub fn save(&self, hosts: &[HostConfig]) -> Result<(), AppError> {
        let content = serde_json::to_string_pretty(hosts).map_err(|error| {
            AppError::StorageError(format!("Failed to serialize hosts: {error}"))
        })?;

        fs::write(&self.file_path, content).map_err(|error| {
            AppError::StorageError(format!("Failed to write hosts file: {error}"))
        })?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::HostStore;
    use crate::models::host::{AuthType, HostConfig, SaveHostRequest};
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
            password_ref: Some("titanssh:host-1:password".to_string()),
            private_key_path: None,
            passphrase_ref: None,
            remark: Some("primary".to_string()),
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
        assert!(error.to_string().contains("Failed to parse hosts file"));
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
            arb_nonempty_string(), // id
            arb_nonempty_string(), // name
            arb_nonempty_string(), // host
            1u16..=65535u16,       // port
            arb_nonempty_string(), // username
            arb_auth_type(),       // auth_type
            proptest::option::of(arb_nonempty_string()), // private_key_path
            proptest::option::of(arb_nonempty_string()), // remark
        )
            .prop_map(
                |(id, name, host, port, username, auth_type, private_key_path, remark)| {
                    // 敏感字段仅以引用键形式存在，格式为 titanssh:<id>:<field>
                    let password_ref = if auth_type == AuthType::Password {
                        Some(format!("titanssh:{}:password", id))
                    } else {
                        None
                    };
                    let passphrase_ref = if auth_type == AuthType::PrivateKey {
                        Some(format!("titanssh:{}:passphrase", id))
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
                    }
                },
            )
    }

    /// 生成非空凭据字符串的策略（至少8个字符，最多24个字符，纯小写字母）
    /// 使用固定前缀 "TESTPWD__" 确保凭据字符串足够独特，不会与其他字段值（如 username、id 等）产生误匹配
    /// 其他字段使用 arb_nonempty_string（大写字母/数字/特殊字符），不会包含此前缀
    fn arb_credential_string() -> impl Strategy<Value = String> {
        "[a-z]{8,24}".prop_map(|s| format!("TESTPWD__{}", s))
    }

    /// 生成含明文密码的 SaveHostRequest 策略（密码认证模式）
    /// - password 字段非空，用于验证落盘后文件中不含该明文密码
    fn arb_password_save_request() -> impl Strategy<Value = SaveHostRequest> {
        (
            arb_nonempty_string(), // id
            arb_nonempty_string(), // name
            arb_nonempty_string(), // host
            1u16..=65535u16,       // port
            arb_nonempty_string(), // username
            arb_credential_string(), // password（明文，不得落盘）
        )
            .prop_map(|(id, name, host, port, username, password)| SaveHostRequest {
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
            })
    }

    /// 生成含明文口令的 SaveHostRequest 策略（私钥认证模式）
    /// - passphrase 字段非空，用于验证落盘后文件中不含该明文口令
    fn arb_passphrase_save_request() -> impl Strategy<Value = SaveHostRequest> {
        (
            arb_nonempty_string(), // id
            arb_nonempty_string(), // name
            arb_nonempty_string(), // host
            1u16..=65535u16,       // port
            arb_nonempty_string(), // username
            arb_nonempty_string(), // private_key_path
            arb_credential_string(), // passphrase（明文，不得落盘）
        )
            .prop_map(|(id, name, host, port, username, private_key_path, passphrase)| {
                SaveHostRequest {
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
                }
            })
    }

    /// 生成同时含明文密码和口令的 SaveHostRequest 策略
    /// - password 和 passphrase 均非空，验证两者均不出现在落盘文件中
    fn arb_both_credentials_save_request() -> impl Strategy<Value = SaveHostRequest> {
        (
            arb_nonempty_string(), // id
            arb_nonempty_string(), // name
            arb_nonempty_string(), // host
            1u16..=65535u16,       // port
            arb_nonempty_string(), // username
            arb_credential_string(), // password（明文，不得落盘）
            arb_nonempty_string(), // private_key_path
            arb_credential_string(), // passphrase（明文，不得落盘）
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

    /// 模拟 save_host 命令中的凭据剥离逻辑：
    /// 将 SaveHostRequest 转换为不含明文凭据的 HostConfig，
    /// 明文密码/口令替换为安全存储引用键（格式：titanssh:<id>:<field>）
    /// 此函数复现 commands/host.rs 中 save_host 的核心落盘逻辑，用于测试隔离
    fn build_host_config_without_plaintext(request: &SaveHostRequest) -> HostConfig {
        // 若存在非空密码，生成引用键；否则为 None
        let password_ref = request
            .password
            .as_deref()
            .filter(|p| !p.is_empty())
            .map(|_| format!("titanssh:{}:password", request.id));

        // 若存在非空口令，生成引用键；否则为 None
        let passphrase_ref = request
            .passphrase
            .as_deref()
            .filter(|p| !p.is_empty())
            .map(|_| format!("titanssh:{}:passphrase", request.id));

        HostConfig {
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
        }
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

            // 验证敏感字段仅以引用形式存在（不含明文密码或口令）
            // password_ref 若存在，必须是引用键格式（以 "titanssh:" 开头），不得是明文密码
            if let Some(ref pw_ref) = loaded_host.password_ref {
                prop_assert!(
                    pw_ref.starts_with("titanssh:"),
                    "password_ref 必须是引用键格式，不得含明文密码，实际值: {}",
                    pw_ref
                );
            }
            // passphrase_ref 若存在，必须是引用键格式，不得是明文口令
            if let Some(ref pp_ref) = loaded_host.passphrase_ref {
                prop_assert!(
                    pp_ref.starts_with("titanssh:"),
                    "passphrase_ref 必须是引用键格式，不得含明文口令，实际值: {}",
                    pp_ref
                );
            }

            // 验证敏感字段引用与原始一致（引用键本身应被正确持久化）
            prop_assert_eq!(&loaded_host.password_ref, &host.password_ref, "password_ref 应一致");
            prop_assert_eq!(&loaded_host.passphrase_ref, &host.passphrase_ref, "passphrase_ref 应一致");
        }

        /// **验证: 需求 1.1, 2.1, 3.1**
        ///
        /// Property 3: hosts.json 不含明文凭据（密码认证模式）
        ///
        /// 使用 proptest 生成含非空明文密码的 SaveHostRequest，模拟 save_host 落盘逻辑后，
        /// 读取 hosts.json 原始文件内容（字符串形式），断言：
        /// 1. 文件内容中不包含原始明文密码字符串
        #[test]
        fn prop_hosts_json_no_plaintext_password(request in arb_password_save_request()) {
            // 提取明文密码，用于后续断言
            let plaintext_password = request.password.clone().unwrap();

            // 模拟 save_host 的凭据剥离逻辑：构建不含明文的 HostConfig
            let host_config = build_host_config_without_plaintext(&request);

            // 使用临时文件路径隔离测试，避免测试间干扰
            let file_path = temp_hosts_file();
            let store = HostStore::from_file_path(file_path.clone());

            // 将不含明文的 HostConfig 写入 hosts.json
            store.save(&[host_config]).expect("save 应成功");

            // 读取 hosts.json 原始文件内容（字符串形式）
            let raw_content = fs::read_to_string(&file_path)
                .expect("hosts.json 应可读取");

            // 断言：文件原始内容中不得包含明文密码字符串
            prop_assert!(
                !raw_content.contains(&plaintext_password),
                "hosts.json 不得包含明文密码，密码: {:?}，文件内容: {}",
                plaintext_password,
                raw_content
            );
        }

        /// **验证: 需求 1.1, 2.1, 3.1**
        ///
        /// Property 3: hosts.json 不含明文凭据（私钥口令模式）
        ///
        /// 使用 proptest 生成含非空明文口令的 SaveHostRequest，模拟 save_host 落盘逻辑后，
        /// 读取 hosts.json 原始文件内容（字符串形式），断言：
        /// 1. 文件内容中不包含原始明文口令字符串
        #[test]
        fn prop_hosts_json_no_plaintext_passphrase(request in arb_passphrase_save_request()) {
            // 提取明文口令，用于后续断言
            let plaintext_passphrase = request.passphrase.clone().unwrap();

            // 模拟 save_host 的凭据剥离逻辑：构建不含明文的 HostConfig
            let host_config = build_host_config_without_plaintext(&request);

            // 使用临时文件路径隔离测试，避免测试间干扰
            let file_path = temp_hosts_file();
            let store = HostStore::from_file_path(file_path.clone());

            // 将不含明文的 HostConfig 写入 hosts.json
            store.save(&[host_config]).expect("save 应成功");

            // 读取 hosts.json 原始文件内容（字符串形式）
            let raw_content = fs::read_to_string(&file_path)
                .expect("hosts.json 应可读取");

            // 断言：文件原始内容中不得包含明文口令字符串
            prop_assert!(
                !raw_content.contains(&plaintext_passphrase),
                "hosts.json 不得包含明文口令，口令: {:?}，文件内容: {}",
                plaintext_passphrase,
                raw_content
            );
        }

        /// **验证: 需求 1.1, 2.1, 3.1**
        ///
        /// Property 3: hosts.json 不含明文凭据（密码与口令同时存在）
        ///
        /// 使用 proptest 生成同时含明文密码和口令的 SaveHostRequest，模拟 save_host 落盘逻辑后，
        /// 读取 hosts.json 原始文件内容（字符串形式），断言：
        /// 1. 文件内容中不包含原始明文密码字符串
        /// 2. 文件内容中不包含原始明文口令字符串
        #[test]
        fn prop_hosts_json_no_plaintext_both_credentials(request in arb_both_credentials_save_request()) {
            // 提取明文密码和口令，用于后续断言
            let plaintext_password = request.password.clone().unwrap();
            let plaintext_passphrase = request.passphrase.clone().unwrap();

            // 模拟 save_host 的凭据剥离逻辑：构建不含明文的 HostConfig
            let host_config = build_host_config_without_plaintext(&request);

            // 使用临时文件路径隔离测试，避免测试间干扰
            let file_path = temp_hosts_file();
            let store = HostStore::from_file_path(file_path.clone());

            // 将不含明文的 HostConfig 写入 hosts.json
            store.save(&[host_config]).expect("save 应成功");

            // 读取 hosts.json 原始文件内容（字符串形式）
            let raw_content = fs::read_to_string(&file_path)
                .expect("hosts.json 应可读取");

            // 断言：文件原始内容中不得包含明文密码字符串
            prop_assert!(
                !raw_content.contains(&plaintext_password),
                "hosts.json 不得包含明文密码，密码: {:?}，文件内容: {}",
                plaintext_password,
                raw_content
            );

            // 断言：文件原始内容中不得包含明文口令字符串
            prop_assert!(
                !raw_content.contains(&plaintext_passphrase),
                "hosts.json 不得包含明文口令，口令: {:?}，文件内容: {}",
                plaintext_passphrase,
                raw_content
            );
        }
    }
}
