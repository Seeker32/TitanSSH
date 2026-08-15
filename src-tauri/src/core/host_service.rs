use crate::core::host_identity::HostIdentityService;
use crate::errors::app_error::AppError;
use crate::models::host::{AuthType, HostConfig, SaveHostRequest};
use crate::storage::host_store::HostStore;
use crate::storage::secure_store;
use tauri::{AppHandle, Manager};

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

/// 真实凭据存储：包装 OS secure storage
pub struct SecureCredentialStore;

impl CredentialStore for SecureCredentialStore {
    fn set(&self, key: &str, value: &str) -> Result<(), AppError> {
        secure_store::set_credential(key, value)
    }

    fn delete(&self, key: &str) -> Result<(), AppError> {
        secure_store::delete_credential(key)
    }
}

/// 信任记录清理 adapter seam：HostConfig 生命周期管理只依赖此 trait 移除
/// endpoint 的持久化信任记录。
///
/// 真实实现包装 HostIdentityService（持久化信任的单一权威，与其共享同一
/// TrustStore 实例，避免第二个实例的独立缓存破坏读写一致性）；
/// 内存实现仅用于测试，可观察调用并注入失败。
pub trait TrustRecordCleanup {
    /// 移除精确 endpoint 的信任记录；endpoint 无记录时幂等成功。
    /// 失败时上层以 HostTrustCleanupFailed 显式上报管理动作未完成。
    fn forget_endpoint(&self, host: &str, port: u16) -> Result<(), AppError>;
}

/// 真实信任记录清理：委托 HostIdentityService 移除 endpoint 的持久化信任记录。
pub struct IdentityTrustCleanup {
    identity_service: HostIdentityService,
}

impl TrustRecordCleanup for IdentityTrustCleanup {
    /// 委托 HostIdentityService 移除精确 endpoint 的持久化信任记录；
    /// 无记录时幂等成功，写入失败时透传结构化错误。
    fn forget_endpoint(&self, host: &str, port: u16) -> Result<(), AppError> {
        self.identity_service.forget_endpoint(host, port)
    }
}

/// HostConfig 持久化 deep module
///
/// 拥有主机配置保存/删除的完整业务流程：请求校验、凭据写入与引用解析、
/// auth type 切换的陈旧凭据清理、endpoint 变更后的信任记录生命周期清理，
/// 以及失败补偿。hosts.json（HostStore）、OS 安全存储（CredentialStore）与
/// 信任记录清理（TrustRecordCleanup）是内部 adapter，不向 command 层泄漏。
pub struct HostConfigService {
    host_store: HostStore,
    credential_store: Box<dyn CredentialStore>,
    trust_cleanup: Box<dyn TrustRecordCleanup>,
}

impl HostConfigService {
    /// 生产构造：从 AppHandle 内建文件存储、真实凭据存储与信任记录清理
    ///
    /// 信任清理复用应用级 HostIdentityService（与连接校验共享同一 TrustStore
    /// 实例），保证清理读写的缓存一致性。
    ///
    /// # 参数
    /// - `app`: Tauri 应用句柄，用于解析 hosts.json 所在的应用数据目录与
    ///   获取受管 HostIdentityService 状态
    ///
    /// # 副作用
    /// 解析并创建应用数据目录；失败返回 StorageError
    pub fn new(app: &AppHandle) -> Result<Self, AppError> {
        let identity_service = app.state::<HostIdentityService>().inner().clone();
        Ok(Self {
            host_store: HostStore::new(app)?,
            credential_store: Box::new(SecureCredentialStore),
            trust_cleanup: Box::new(IdentityTrustCleanup { identity_service }),
        })
    }

    /// 测试构造：注入文件存储、凭据存储与信任清理，覆盖成功与失败路径
    #[cfg(test)]
    fn with_stores(
        host_store: HostStore,
        credential_store: Box<dyn CredentialStore>,
        trust_cleanup: Box<dyn TrustRecordCleanup>,
    ) -> Self {
        Self {
            host_store,
            credential_store,
            trust_cleanup,
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

    /// 保存主机配置，返回更新后的完整列表
    ///
    /// 流程：校验 → 加载现有列表 → 写入新凭据并解析引用（留空/缺省且 auth type
    /// 未变时保留旧引用）→ 构造 HostConfig → upsert → 落盘（commit 点）→
    /// auth type 切换时尽力删除陈旧凭据 → endpoint 变化时清理旧信任记录。
    ///
    /// # 一致性保证
    /// 任一失败（凭据写入或落盘）时，补偿删除本次调用已写入的凭据，保持
    /// 安全存储与 hosts.json 一致；陈旧凭据清理只发生在 commit 之后，
    /// 失败时旧凭据保持可用。信任记录清理同样只发生在 commit 之后：
    /// 仅当更新后的配置集合不再引用旧 endpoint 时才删除其记录；清理失败
    /// 以 HostTrustCleanupFailed 显式返回（commit 已生效，不补偿已写入凭据）。
    ///
    /// # 参数
    /// - `request`: 含明文凭据的保存请求，处理完毕后明文不持久化
    pub fn save(&self, request: &SaveHostRequest) -> Result<Vec<HostConfig>, AppError> {
        validate_save_request(request)?;

        // 本次调用已写入的凭据 key；任一失败时用于补偿删除，保持安全存储与文件一致
        let mut written_keys: Vec<String> = Vec::new();

        let result = (|| -> Result<SaveResult, AppError> {
            let existing_hosts = self.host_store.load()?;
            let existing = existing_hosts.iter().find(|host| host.id == request.id);
            // 认证方式切换时，旧凭据不再适用，不得保留其引用
            let auth_type_changed = existing
                .map(|host| host.auth_type != request.auth_type)
                .unwrap_or(false);

            // endpoint 发生变化的旧值；commit 成功后若不再被引用则清理其信任记录
            // 精确 host 字符串 + port 比较，不做任何归一化
            let old_endpoint = existing
                .filter(|host| host.host != request.host || host.port != request.port)
                .map(|host| (host.host.clone(), host.port));

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
                group: request.group.clone(),
            };

            // 复用已加载的主机列表，避免重复读取文件
            let mut hosts = existing_hosts;
            if let Some(index) = hosts.iter().position(|item| item.id == host_config.id) {
                hosts[index] = host_config;
            } else {
                hosts.push(host_config);
            }

            // 落盘是 commit 点：文件更新成功后，才清理切换遗留的陈旧凭据
            self.host_store.save(&hosts)?;
            if auth_type_changed {
                if request.auth_type == AuthType::PrivateKey {
                    // 旧密码不再适用，尽力删除（失败只留孤儿条目，不阻断保存）
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
            Ok((hosts, old_endpoint))
        })();

        match result {
            Ok((hosts, old_endpoint)) => {
                // commit 已生效：信任记录清理失败必须显式上报，不做凭据补偿
                self.cleanup_unreferenced_endpoint(&hosts, old_endpoint)?;
                Ok(hosts)
            }
            Err(error) => {
                // 失败补偿：删除本次调用写入的凭据，避免孤儿条目与悬空引用
                for key in &written_keys {
                    let _ = self.credential_store.delete(key);
                }
                Err(error)
            }
        }
    }

    /// 删除主机配置，返回更新后的完整列表
    ///
    /// 先落盘移除条目（commit 点），成功后再尽力删除密码与口令两个凭据 key，
    /// 最后在被删 endpoint 不再被剩余配置引用时清理其信任记录。
    /// 凭据删除失败不阻断主机删除（最多留下孤儿 keychain 条目）；
    /// 信任清理失败以 HostTrustCleanupFailed 显式返回（删除已生效）。
    ///
    /// # 参数
    /// - `host_id`: 要删除的主机 ID
    pub fn delete(&self, host_id: &str) -> Result<Vec<HostConfig>, AppError> {
        let mut hosts = self.host_store.load()?;
        // 被删主机的精确 endpoint（host 字符串 + port）；不存在时无清理目标
        let removed_endpoint = hosts
            .iter()
            .find(|host| host.id == host_id)
            .map(|host| (host.host.clone(), host.port));
        hosts.retain(|host| host.id != host_id);

        // 落盘是 commit 点：失败时凭据保持原样
        self.host_store.save(&hosts)?;

        // 无条件清理两个凭据 key（幂等，兼容切换清理上线前的遗留数据）
        let _ = self
            .credential_store
            .delete(&secure_store::password_key(host_id));
        let _ = self
            .credential_store
            .delete(&secure_store::passphrase_key(host_id));

        // commit 已生效：信任记录清理失败必须显式上报
        self.cleanup_unreferenced_endpoint(&hosts, removed_endpoint)?;
        Ok(hosts)
    }

    /// commit 成功后清理不再被引用的旧 endpoint 信任记录。
    ///
    /// 仅当新配置集合中不存在精确 host 字符串 + port 引用时执行；共享 endpoint
    /// 或未变化的 endpoint 不清理。清理失败包装为 HostTrustCleanupFailed
    /// 显式返回：管理动作未完成时不得静默报告为成功。
    ///
    /// # 参数
    /// - `hosts`: commit 后的配置集合（引用判断依据）
    /// - `old_endpoint`: 变更前的 endpoint（None 表示本次无 endpoint 变化）
    fn cleanup_unreferenced_endpoint(
        &self,
        hosts: &[HostConfig],
        old_endpoint: Option<(String, u16)>,
    ) -> Result<(), AppError> {
        if let Some((host, port)) = old_endpoint {
            let still_referenced = hosts
                .iter()
                .any(|item| item.host == host && item.port == port);
            if !still_referenced {
                self.trust_cleanup
                    .forget_endpoint(&host, port)
                    .map_err(|error| {
                        AppError::HostTrustCleanupFailed(format!(
                            "endpoint {host}:{port} 的信任记录清理失败: {error}"
                        ))
                    })?;
            }
        }
        Ok(())
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

/// save 内部流程结果：commit 后的配置列表 + 本次发生变化的旧 endpoint。
type SaveResult = (Vec<HostConfig>, Option<(String, u16)>);

/// 验证保存主机请求的必填字段，name/host/username 不得为空白
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
#[path = "host_service_test.rs"]
mod tests;
