use crate::core::host_identity::HostIdentityService;
use crate::errors::app_error::{AppError, ErrorDetail};
use crate::models::host::{AuthType, CredentialInput, HostConfig, SaveHostRequest};
use crate::storage::host_store::HostStore;
use crate::storage::secure_store;
use std::sync::Mutex;
use tauri::{AppHandle, Manager};

/// 凭据存储 adapter seam:HostConfig 持久化 module 只依赖此 trait 访问 OS 安全存储
///
/// 真实实现包装 secure_store(Keychain / Credential Manager / Secret Service);
/// 内存实现仅用于测试,可注入失败以覆盖错误路径。
pub trait CredentialStore {
    /// 读取凭据;不存在的 key 返回 CredentialNotFound
    fn get(&self, key: &str) -> Result<String, AppError>;

    /// 写入凭据;写入失败时上层负责补偿
    fn set(&self, key: &str, value: &str) -> Result<(), AppError>;

    /// 删除凭据;不存在的 key 静默成功
    fn delete(&self, key: &str) -> Result<(), AppError>;
}

/// 真实凭据存储：包装 OS secure storage
pub struct SecureCredentialStore;

impl CredentialStore for SecureCredentialStore {
    fn get(&self, key: &str) -> Result<String, AppError> {
        secure_store::get_credential(key)
    }

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
    credential_store: Box<dyn CredentialStore + Send + Sync>,
    trust_cleanup: Box<dyn TrustRecordCleanup + Send + Sync>,
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
        credential_store: Box<dyn CredentialStore + Send + Sync>,
        trust_cleanup: Box<dyn TrustRecordCleanup + Send + Sync>,
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
    /// 流程：校验 → 加载现有列表 → 写入与 auth type 相关的凭据并解析三态输入
    /// （Keep/Set/Clear：无关凭据不写入、引用置 None；相关凭据 Keep 且 auth type
    /// 未变时保留旧引用）→ 构造 HostConfig → upsert → 落盘（commit 点）→
    /// 尽力删除显式 Clear 与 auth type 切换遗留的陈旧凭据 →
    /// endpoint 变化时清理旧信任记录。
    ///
    /// # 一致性保证
    /// 任一失败（凭据写入或落盘）时，补偿将本次调用已写入的凭据还原到写入前状态：
    /// 覆盖写入的还原旧值、新增的删除，保持安全存储与 hosts.json 一致——
    /// 已存主机覆盖保存失败时旧凭据仍被未修改的 hosts.json 引用，必须还原而非删除；
    /// 陈旧凭据清理只发生在 commit 之后，失败时旧凭据保持可用。信任记录清理同样
    /// 只发生在 commit 之后：仅当更新后的配置集合不再引用旧 endpoint 时才删除其记录；
    /// 清理失败以 HostTrustCleanupFailed 显式返回（commit 已生效，不补偿已写入凭据）。
    ///
    /// # 参数
    /// - `request`: 含明文凭据的保存请求，处理完毕后明文不持久化
    pub fn save(&self, request: &SaveHostRequest) -> Result<Vec<HostConfig>, AppError> {
        validate_save_request(request)?;

        // 本次调用已写入的凭据及其写入前快照；任一失败时用于补偿还原，保持安全存储与文件一致
        let mut written: Vec<CredentialWrite> = Vec::new();
        // 显式 Clear 的凭据 key；commit 成功后才尽力删除，失败时旧凭据保持可用
        let mut clear_keys: Vec<String> = Vec::new();

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

            // 仅解析与 auth type 相关的凭据：无关凭据不写入安全存储、引用置 None。
            // 若两种都写，auth 切换后的陈旧清理会误删本次调用刚写入的 key，
            // 使落盘引用悬空；只写相关的那一个则被清理的必非本次写入。
            let (password_ref, passphrase_ref) = if request.auth_type == AuthType::Password {
                let password_ref = self.resolve_credential_ref(
                    &request.id,
                    &request.password,
                    secure_store::password_key,
                    existing.and_then(|host| host.password_ref.as_deref()),
                    auth_type_changed,
                    &mut written,
                    &mut clear_keys,
                )?;
                (password_ref, None)
            } else {
                let passphrase_ref = self.resolve_credential_ref(
                    &request.id,
                    &request.passphrase,
                    secure_store::passphrase_key,
                    existing.and_then(|host| host.passphrase_ref.as_deref()),
                    auth_type_changed,
                    &mut written,
                    &mut clear_keys,
                )?;
                (None, passphrase_ref)
            };

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

            // 落盘是 commit 点：文件更新成功后，才清理显式清除与切换遗留的陈旧凭据。
            // 被清理的是与旧 auth type 相关、本次未写入的 key（本调用只写与新
            // auth type 相关的凭据），不存在误删本次写入导致落盘引用悬空。
            self.host_store.save(&hosts)?;
            for key in &clear_keys {
                // 显式 Clear：引用已置 None，尽力删除已存凭据（失败只留孤儿条目，不阻断保存）
                let _ = self.credential_store.delete(key);
            }
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
                // 失败补偿：还原被覆盖的旧凭据、删除本次新增的凭据，避免孤儿条目与悬空引用。
                // 覆盖保存已有主机时 key 与旧 hosts.json 引用相同，直接删除会永久丢失旧凭据。
                for write in &written {
                    match &write.prev {
                        Some(prev) => {
                            let _ = self.credential_store.set(&write.key, prev);
                        }
                        None => {
                            let _ = self.credential_store.delete(&write.key);
                        }
                    }
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
                        AppError::HostTrustCleanupFailed(ErrorDetail::msg(
                            "endpoint {0}:{1} 的信任记录清理失败: {2}",
                            vec![host, port.to_string(), error.to_string()],
                        ))
                    })?;
            }
        }
        Ok(())
    }

    /// 解析与 auth type 相关的单个三态凭据输入（调用方只对相关凭据调用本方法）:
    /// - Set(非空)写入安全存储并返回新引用 key；空串等价于 Keep（兼容旧前端「留空则保持」）
    /// - Keep 时认证方式未切换则保留旧引用，切换则置 None
    /// - Clear 置 None 并把目标 key 记入 clear_keys，由 save 在 commit 后尽力删除
    ///
    /// # 参数
    /// - `host_id`: 主机 ID,用于派生引用 key
    /// - `provided`: 三态凭据输入
    /// - `key_fn`: 引用 key 生成函数(password_key / passphrase_key)
    /// - `existing_ref`: 旧引用
    /// - `auth_type_changed`: 认证方式是否已切换
    /// - `written`: 本次调用已写入的凭据及写入前快照,供失败补偿还原
    /// - `clear_keys`: 本次调用显式 Clear 的 key,commit 成功后由 save 尽力删除
    fn resolve_credential_ref(
        &self,
        host_id: &str,
        provided: &Option<CredentialInput>,
        key_fn: fn(&str) -> String,
        existing_ref: Option<&str>,
        auth_type_changed: bool,
        written: &mut Vec<CredentialWrite>,
        clear_keys: &mut Vec<String>,
    ) -> Result<Option<String>, AppError> {
        match provided {
            Some(CredentialInput::Set(value)) if !value.is_empty() => {
                let key = key_fn(host_id);
                // 覆盖前快照旧值：key 由 host_id 派生，与旧 hosts.json 引用相同，
                // 失败补偿时需还原旧值而非删除；快照失败立即中止，避免盲覆盖破坏旧凭据
                let prev = match self.credential_store.get(&key) {
                    Ok(value) => Some(value),
                    Err(AppError::CredentialNotFound(_)) => None,
                    Err(error) => return Err(error),
                };
                self.credential_store.set(&key, value)?;
                written.push(CredentialWrite {
                    key: key.clone(),
                    prev,
                });
                Ok(Some(key))
            }
            Some(CredentialInput::Clear { .. }) => {
                // 显式清除：引用置 None；已存凭据由 save 在 commit 后尽力删除，
                // 不在此处删除——commit 失败时旧 hosts.json 仍引用该 key
                clear_keys.push(key_fn(host_id));
                Ok(None)
            }
            // Keep 或空串 Set（旧前端「留空则保持」语义）
            _ => {
                if auth_type_changed {
                    // 切换认证方式:旧引用不再适用,置 None
                    Ok(None)
                } else {
                    Ok(existing_ref.map(String::from))
                }
            }
        }
    }
}

/// save 内部流程结果：commit 后的配置列表 + 本次发生变化的旧 endpoint。
type SaveResult = (Vec<HostConfig>, Option<(String, u16)>);

/// 本次调用写入凭据的补偿信息：key + 覆盖前的旧值快照。
/// prev 为 None 表示写入前该 key 不存在，补偿时删除；Some 则补偿时还原旧值。
struct CredentialWrite {
    key: String,
    prev: Option<String>,
}

/// 应用级共享服务：单一实例持有 Mutex，串行化 hosts.json 的读-改-写周期
///
/// 命令在 spawn_blocking 线程池并发执行时，list/save/delete 必须全部经过
/// 同一实例持锁操作；否则并发 load-modify-write 互相覆盖，后写者丢弃先写者
/// 的更新（已删主机会重现、已存主机会消失、keyring 凭据会被孤儿化）。
/// 锁中毒时恢复内部状态继续：服务本身无跨调用不变量，后续操作重新从文件加载。
pub struct SharedHostConfigService {
    inner: Mutex<HostConfigService>,
}

impl SharedHostConfigService {
    /// 生产构造：复用应用级 HostIdentityService，与连接校验共享同一 TrustStore 实例
    pub fn new(app: &AppHandle) -> Result<Self, AppError> {
        Ok(Self {
            inner: Mutex::new(HostConfigService::new(app)?),
        })
    }

    /// 持锁执行一次完整业务操作（load-modify-write 全程在锁内）；
    /// 锁中毒时恢复内部状态继续，避免一次 panic 永久阻断主机管理
    pub fn with_locked<T>(
        &self,
        func: impl FnOnce(&HostConfigService) -> Result<T, AppError>,
    ) -> Result<T, AppError> {
        let guard = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        func(&guard)
    }

    /// 测试构造：包装已注入 adapter 的服务
    #[cfg(test)]
    fn from_service(service: HostConfigService) -> Self {
        Self {
            inner: Mutex::new(service),
        }
    }
}

/// 验证保存主机请求的必填字段：name/host/username 不得为空白，
/// 端口必须在 1-65535 之间，PrivateKey 认证必须提供非空私钥路径
fn validate_save_request(request: &SaveHostRequest) -> Result<(), AppError> {
    if request.name.trim().is_empty() {
        return Err(AppError::InvalidHostConfig(ErrorDetail::msg(
            "主机名称为必填项",
            Vec::new(),
        )));
    }
    if request.host.trim().is_empty() {
        return Err(AppError::InvalidHostConfig(ErrorDetail::msg(
            "主机地址为必填项",
            Vec::new(),
        )));
    }
    if request.username.trim().is_empty() {
        return Err(AppError::InvalidHostConfig(ErrorDetail::msg(
            "用户名为必填项",
            Vec::new(),
        )));
    }
    if request.port == 0 {
        return Err(AppError::InvalidHostConfig(ErrorDetail::msg(
            "端口号必须在 1-65535 之间",
            Vec::new(),
        )));
    }
    if request.auth_type == AuthType::PrivateKey
        && request
            .private_key_path
            .as_deref()
            .map_or(true, |path| path.trim().is_empty())
    {
        return Err(AppError::InvalidHostConfig(ErrorDetail::msg(
            "私钥认证必须提供私钥文件路径",
            Vec::new(),
        )));
    }
    Ok(())
}

#[cfg(test)]
#[path = "host_service_test.rs"]
mod tests;
