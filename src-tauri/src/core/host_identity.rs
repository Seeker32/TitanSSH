//! 主机身份确认（TOFU，仅本次接受）：临时信任、pending challenge 与等待者的单一后端权威。
//!
//! 所有 capability（Terminal、SFTP、Monitoring）的 SSH 连接在握手后、认证前
//! 通过 [`HostKeyVerifier`] 进入统一校验；未知主机产生一个 challenge 事件并阻塞
//! 等待用户决定。信任以 Runtime Session 为作用域：同一 Session 内接受过的
//! endpoint+指纹后续连接（含重连）直接放行，Session 关闭即清除。
//! 不把策略复制到各 capability service。

use crate::errors::app_error::{AppError, ErrorDetail};
use crate::models::host_identity::TrustedHostInfo;
use crate::models::session::{HostIdentityChallenge, HostIdentityChallengeKind};
use crate::storage::trust_store::{TrustRecord, TrustStore};
use base64::Engine;
use base64::engine::general_purpose::STANDARD_NO_PAD;
use sha2::{Digest, Sha256};
use ssh2::HostKeyType;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use tauri::{AppHandle, Emitter, Runtime};
use uuid::Uuid;

/// transport 在握手后、认证前采集到的主机公钥呈现信息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresentedHostKey {
    pub host: String,
    pub port: u16,
    /// OpenSSH 风格算法名（如 ssh-ed25519）
    pub algorithm: String,
    /// OpenSSH 风格 SHA-256 指纹（SHA256:base64 无填充）
    pub fingerprint: String,
    /// ssh2 提供的原始主机公钥 blob（OpenSSH wire 格式）；"接受并保存"持久化用，
    /// 绝不进入 challenge 事件 payload（前端只看到指纹）
    pub blob: Vec<u8>,
}

/// 统一校验入口：由 HostIdentityService 按 session 构建，注入 transport。
pub type HostKeyVerifier = Arc<dyn Fn(&PresentedHostKey) -> Result<(), AppError> + Send + Sync>;

/// 计算 OpenSSH 风格 SHA-256 指纹：`SHA256:<base64 无填充>`。
pub fn fingerprint_sha256(blob: &[u8]) -> String {
    let digest = Sha256::digest(blob);
    format!("SHA256:{}", STANDARD_NO_PAD.encode(digest))
}

/// 将 ssh2 主机密钥类型映射为 OpenSSH 风格算法名。
pub fn algorithm_name(key_type: HostKeyType) -> &'static str {
    match key_type {
        HostKeyType::Rsa => "ssh-rsa",
        HostKeyType::Dss => "ssh-dss",
        HostKeyType::Ecdsa256 => "ecdsa-sha2-nistp256",
        HostKeyType::Ecdsa384 => "ecdsa-sha2-nistp384",
        HostKeyType::Ecdsa521 => "ecdsa-sha2-nistp521",
        HostKeyType::Ed25519 => "ssh-ed25519",
        HostKeyType::Unknown => "unknown",
    }
}

/// challenge 的最终决定。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Decision {
    Accepted,
    Rejected,
    Cancelled,
}

/// 一个 pending challenge：事件 payload + 等待者的唤醒点。
/// decision 单独加锁，等待者不持有服务级状态锁。
struct ChallengeWait {
    challenge: HostIdentityChallenge,
    /// challenge 创建时快照的完整公钥 blob："接受并保存"持久化使用，不发送给前端
    presented_blob: Vec<u8>,
    decision: Mutex<Option<Decision>>,
    signal: Condvar,
    /// 当前等待该 challenge 的连接数（诊断与测试观察合并进度）
    waiting: AtomicUsize,
}

/// 信任与 pending 的复合键：Runtime Session + endpoint + 指纹。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct IdentityKey {
    session_id: String,
    host: String,
    port: u16,
    fingerprint: String,
}

struct IdentityState {
    /// 已在本 Runtime Session 接受的 endpoint+指纹（仅本次信任，不落盘）
    trusted: HashSet<IdentityKey>,
    /// pending challenge，键为 challenge_id
    pending: HashMap<String, Arc<ChallengeWait>>,
    /// 并发合并索引：同一 Session、endpoint 与指纹共用一个 challenge
    pending_index: HashMap<IdentityKey, String>,
    /// 已关闭（或应用退出）的 Session：迟到到达的校验器立即失败，
    /// 不再创建无人取消的 pending challenge（等待者不得永久阻塞）
    cancelled: HashSet<String>,
}

impl IdentityKey {
    /// 从 challenge 派生复合键（Session + endpoint + 指纹），accept/取消路径复用。
    fn from_challenge(challenge: &HostIdentityChallenge) -> Self {
        Self {
            session_id: challenge.session_id.clone(),
            host: challenge.host.clone(),
            port: challenge.port,
            fingerprint: challenge.fingerprint.clone(),
        }
    }
}

/// 主机身份确认服务：临时信任、pending challenge 与等待者的后端权威，
/// 并持有 TitanSSH 独立信任存储（应用数据目录下的 known_hosts）。
#[derive(Clone)]
pub struct HostIdentityService {
    state: Arc<Mutex<IdentityState>>,
    /// 持久化信任存储；None 表示未初始化（等价空信任存储，仅测试路径使用）
    trust_store: Arc<Mutex<Option<TrustStore>>>,
}

impl Default for HostIdentityService {
    fn default() -> Self {
        Self::new()
    }
}

impl HostIdentityService {
    /// 构建空状态的主机身份确认服务。
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(IdentityState {
                trusted: HashSet::new(),
                pending: HashMap::new(),
                pending_index: HashMap::new(),
                cancelled: HashSet::new(),
            })),
            trust_store: Arc::new(Mutex::new(None)),
        }
    }

    /// 初始化 TitanSSH 独立信任存储（应用数据目录下的 known_hosts）。
    ///
    /// 应用启动 setup 阶段调用一次；目录创建失败返回 TrustStoreError。
    /// 不读取或写入系统 `~/.ssh/known_hosts`，也不使用 keyring。
    pub fn init_trust_store<R: Runtime>(&self, app: &AppHandle<R>) -> Result<(), AppError> {
        let store = TrustStore::new(app)?;
        *self
            .trust_store
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(store);
        Ok(())
    }

    /// 测试构造：注入指定路径的信任存储。
    #[cfg(test)]
    pub(crate) fn with_trust_store_path(path: std::path::PathBuf) -> Self {
        Self::with_trust_store(TrustStore::from_file_path(path))
    }

    /// 测试构造：注入现成的信任存储实例。
    #[cfg(test)]
    pub(crate) fn with_trust_store(store: TrustStore) -> Self {
        Self {
            state: Arc::new(Mutex::new(IdentityState {
                trusted: HashSet::new(),
                pending: HashMap::new(),
                pending_index: HashMap::new(),
                cancelled: HashSet::new(),
            })),
            trust_store: Arc::new(Mutex::new(Some(store))),
        }
    }

    /// 为指定 Runtime Session 构建统一校验闭包，注入 transport / capability 连接路径。
    pub fn verifier<R: Runtime>(&self, app: AppHandle<R>, session_id: String) -> HostKeyVerifier {
        let service = self.clone();
        Arc::new(move |presented| service.verify(&app, &session_id, presented))
    }

    /// 统一校验：持久化信任精确匹配直接放行并记为 Session 已验证决定；
    /// 已信任直接放行；未知主机或已保存 key 与呈现不一致时派发 challenge 事件并阻塞
    /// 等待用户决定。信任存储不可读/不可解析时 fail-closed。
    /// 已保存记录与呈现 key 任一不一致都产生 Changed challenge（携带旧记录与呈现的
    /// 算法/指纹），绝不覆盖或删除旧记录，也不开始认证。
    /// 同一 Session、endpoint 与指纹的并发连接合并到同一 challenge；
    /// 同一 Session、endpoint 已有 pending challenge 而新呈现指纹不同（服务端在
    /// challenge 后再次更换 key）时，新 challenge 取代旧 challenge：旧等待者取消，
    /// 对旧 challenge 的一切决定安全失败，绝不借旧决定认证新 key。
    /// 已验证决定在信任记录被生命周期清理移除后仍持续到 Session 关闭；
    /// 已关闭 Session 的迟到校验器（含已保存 key 精确匹配）仍必须立即失败，
    /// 不得借持久化信任继续认证。
    pub fn verify<R: Runtime>(
        &self,
        app: &AppHandle<R>,
        session_id: &str,
        presented: &PresentedHostKey,
    ) -> Result<(), AppError> {
        let key = IdentityKey {
            session_id: session_id.to_string(),
            host: presented.host.clone(),
            port: presented.port,
            fingerprint: presented.fingerprint.clone(),
        };
        {
            let state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            // 会话已关闭：迟到校验器（如已发放给 Monitoring worker）立即失败，
            // 不再创建无人取消的 challenge
            if state.cancelled.contains(session_id) {
                return Err(AppError::HostKeyVerificationCancelled(
                    session_id.to_string().into(),
                ));
            }
            if state.trusted.contains(&key) {
                return Ok(());
            }
        }
        // 持久化信任：精确 host+port+算法+完整公钥匹配即静默放行；存储错误 fail-closed。
        // 已保存记录与呈现 key 不一致时快照旧记录（Changed challenge 的展示与替换依据），
        // 不在此处改动旧记录。
        let stored_record: Option<TrustRecord> = if let Some(store) = self
            .trust_store
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
        {
            match store.lookup(&presented.host, presented.port)? {
                Some(record)
                    if record.matches(
                        &presented.host,
                        presented.port,
                        &presented.algorithm,
                        &presented.blob,
                    ) =>
                {
                    // 已验证决定按 Session 记录：信任记录被生命周期清理移除后，
                    // 同 Session 的重连仍静默放行，直到 Session 关闭
                    let mut state = self
                        .state
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    if state.cancelled.contains(session_id) {
                        return Err(AppError::HostKeyVerificationCancelled(
                            session_id.to_string().into(),
                        ));
                    }
                    state.trusted.insert(key);
                    return Ok(());
                }
                mismatch => mismatch,
            }
        } else {
            None
        };
        let (wait, created, superseded) = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            // 两次状态锁之间会话可能被关闭：再次校验，不得为已关闭会话创建 challenge
            if state.cancelled.contains(session_id) {
                return Err(AppError::HostKeyVerificationCancelled(
                    session_id.to_string().into(),
                ));
            }
            if state.trusted.contains(&key) {
                return Ok(());
            }
            match state.pending_index.get(&key) {
                Some(challenge_id) => (
                    state
                        .pending
                        .get(challenge_id)
                        .cloned()
                        .expect("pending_index 与 pending 同步维护"),
                    false,
                    Vec::new(),
                ),
                None => {
                    // 同一 Session、同一 endpoint 已有 pending challenge 但指纹不同：
                    // 服务端在 challenge 之后再次更换 key。旧 challenge 必须被取代——
                    // 旧等待者取消（连接不得以未确认的旧 key 认证），
                    // 对旧 challenge 的后续决定一律 HostKeyChallengeNotFound 安全失败。
                    let superseded: Vec<Arc<ChallengeWait>> = state
                        .pending
                        .values()
                        .filter(|other| {
                            other.challenge.session_id == session_id
                                && other.challenge.host == presented.host
                                && other.challenge.port == presented.port
                                && other.challenge.fingerprint != presented.fingerprint
                        })
                        .cloned()
                        .collect();
                    for old in &superseded {
                        state.pending.remove(&old.challenge.challenge_id);
                        state
                            .pending_index
                            .remove(&IdentityKey::from_challenge(&old.challenge));
                    }
                    let challenge = HostIdentityChallenge {
                        challenge_id: Uuid::new_v4().to_string(),
                        session_id: session_id.to_string(),
                        host: presented.host.clone(),
                        port: presented.port,
                        kind: match &stored_record {
                            Some(_) => HostIdentityChallengeKind::Changed,
                            None => HostIdentityChallengeKind::Unknown,
                        },
                        key_algorithm: presented.algorithm.clone(),
                        fingerprint: presented.fingerprint.clone(),
                        stored_algorithm: stored_record
                            .as_ref()
                            .map(|record| record.algorithm.clone()),
                        stored_fingerprint: stored_record
                            .as_ref()
                            .map(|record| fingerprint_sha256(&record.blob)),
                        timestamp: chrono::Utc::now().timestamp_millis(),
                    };
                    let wait = Arc::new(ChallengeWait {
                        decision: Mutex::new(None),
                        signal: Condvar::new(),
                        waiting: AtomicUsize::new(0),
                        challenge,
                        presented_blob: presented.blob.clone(),
                    });
                    state
                        .pending
                        .insert(wait.challenge.challenge_id.clone(), wait.clone());
                    state
                        .pending_index
                        .insert(key, wait.challenge.challenge_id.clone());
                    (wait, true, superseded)
                }
            }
        };
        // 被取代的旧 challenge 等待者一律取消：绝不借旧决定认证新 key
        for old in superseded {
            Self::decide(&old, Decision::Cancelled);
        }
        // 仅首个到达的连接派发 challenge 事件；合并到同一 challenge 的等待者不重复派发
        if created {
            let _ = app.emit("host-identity:challenge", &wait.challenge);
        }
        Self::wait_for_decision(&wait, session_id)
    }

    /// 阻塞等待决定；不设独立自动超时，由用户决定或会话关闭唤醒。
    fn wait_for_decision(wait: &Arc<ChallengeWait>, session_id: &str) -> Result<(), AppError> {
        wait.waiting.fetch_add(1, Ordering::Relaxed);
        let mut decision = wait
            .decision
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        loop {
            if let Some(decided) = *decision {
                wait.waiting.fetch_sub(1, Ordering::Relaxed);
                return match decided {
                    Decision::Accepted => Ok(()),
                    Decision::Rejected => Err(AppError::HostKeyRejected(
                        format!(
                            "{}:{} ({})",
                            wait.challenge.host, wait.challenge.port, wait.challenge.fingerprint
                        )
                        .into(),
                    )),
                    Decision::Cancelled => Err(AppError::HostKeyVerificationCancelled(
                        session_id.to_string().into(),
                    )),
                };
            }
            decision = wait
                .signal
                .wait(decision)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
    }

    /// 仅本次接受：为该 Runtime Session 记录临时信任并唤醒全部等待者。
    /// pending 移除与信任写入在同一锁内完成：并发连接要么看到信任直接放行，
    /// 要么看到同一 challenge 继续等待，不存在"接受后重复确认"的窗口。
    pub fn accept(&self, challenge_id: &str) -> Result<(), AppError> {
        let wait = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let wait = state.pending.remove(challenge_id).ok_or_else(|| {
                AppError::HostKeyChallengeNotFound(challenge_id.to_string().into())
            })?;
            let key = IdentityKey::from_challenge(&wait.challenge);
            state.pending_index.remove(&key);
            state.trusted.insert(key);
            wait
        };
        Self::decide(&wait, Decision::Accepted);
        Ok(())
    }

    /// 接受并保存/替换记录：把 challenge 快照的算法 + 完整公钥持久化到信任存储，
    /// 然后像 accept 一样记录临时信任并唤醒全部等待者。
    ///
    /// 保存失败时 challenge 保持未决（不授予任何信任，不自动降级为临时信任），
    /// 以 HostKeySaveFailed 结构化返回，用户可重试保存、改选仅本次接受或拒绝。
    /// 保存成功后，其他 Session 中同 endpoint + 同 key 的 pending challenge 一并放行。
    ///
    /// 状态锁覆盖「存在性检查 → 持久化 → 移除」全程：保存期间 challenge 无法被
    /// 取代/拒绝/重复解决，stale 决定（challenge 已不存在）在写盘前即失败，
    /// 绝不把过时 key 写入信任存储。锁顺序为 state → trust store，与 verify 的
    /// 短暂 store 锁（释放后再取 state 锁）不构成环。
    pub fn accept_and_save(&self, challenge_id: &str) -> Result<(), AppError> {
        let (wait, released_others) = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            // challenge 已被取代/拒绝/重复解决：stale 决定在写盘前安全失败
            let wait = state.pending.get(challenge_id).cloned().ok_or_else(|| {
                AppError::HostKeyChallengeNotFound(challenge_id.to_string().into())
            })?;

            // 持久化：trust store 内部串行化读写并安全发布，失败不改动旧记录。
            // 写入失败时本 challenge 尚未从 pending 移除，保持未决。
            let store = self
                .trust_store
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone()
                .ok_or_else(|| {
                    AppError::HostKeySaveFailed(ErrorDetail::msg(
                        "信任存储未初始化，无法持久化信任记录",
                        Vec::new(),
                    ))
                })?;
            store
                .upsert(TrustRecord {
                    host: wait.challenge.host.clone(),
                    port: wait.challenge.port,
                    algorithm: wait.challenge.key_algorithm.clone(),
                    blob: wait.presented_blob.clone(),
                })
                .map_err(|error| AppError::HostKeySaveFailed(error.to_string().into()))?;

            // 移除本 challenge + 写入临时信任；同 endpoint + 同 key 的
            // 其他 Session pending challenge 一并移除（其等待者由持久化信任覆盖）
            state.pending.remove(challenge_id);
            let key = IdentityKey::from_challenge(&wait.challenge);
            state.pending_index.remove(&key);
            state.trusted.insert(key);
            let others: Vec<Arc<ChallengeWait>> = state
                .pending
                .values()
                .filter(|other| {
                    other.challenge.host == wait.challenge.host
                        && other.challenge.port == wait.challenge.port
                        && other.challenge.key_algorithm == wait.challenge.key_algorithm
                        && other.presented_blob == wait.presented_blob
                })
                .cloned()
                .collect();
            for other in &others {
                state.pending.remove(&other.challenge.challenge_id);
                state
                    .pending_index
                    .remove(&IdentityKey::from_challenge(&other.challenge));
            }
            (wait, others)
        };
        Self::decide(&wait, Decision::Accepted);
        for other in released_others {
            Self::decide(&other, Decision::Accepted);
        }
        Ok(())
    }

    /// 移除 endpoint 的持久化信任记录（HostConfig 保存/删除的生命周期清理）。
    ///
    /// 只影响长期信任：运行中 Runtime Session 的临时信任、已验证决定与 pending
    /// challenge 不受影响，已建立的连接继续运行；新 Session 连接该 endpoint 时
    /// 将重新视为未知并触发确认。endpoint 无记录时幂等成功；存储未初始化或
    /// 写入失败返回结构化错误，调用方必须显式上报，不得静默吞掉未完成的清理。
    pub fn forget_endpoint(&self, host: &str, port: u16) -> Result<(), AppError> {
        let store = self
            .trust_store
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
            .ok_or_else(|| {
                AppError::TrustStoreError(ErrorDetail::msg(
                    "信任存储未初始化，无法清理信任记录",
                    Vec::new(),
                ))
            })?;
        store.remove(host, port)
    }

    /// 列出持久化信任记录（endpoint、算法与 SHA-256 指纹），供 Settings 只读清单展示。
    ///
    /// 每条记录按 host 字典序 + port 稳定排序；指纹由后端从完整公钥 blob 计算，
    /// 前端只消费 typed JSON。存储未初始化（仅测试路径）等价空清单；读取或解析
    /// 失败以 TrustStoreError 显式返回，绝不伪装成空列表。
    pub fn list_trusted_hosts(&self) -> Result<Vec<TrustedHostInfo>, AppError> {
        let store = self
            .trust_store
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let Some(store) = store else {
            return Ok(Vec::new());
        };
        store.list().map(|records| {
            records
                .into_iter()
                .map(|record| TrustedHostInfo {
                    host: record.host,
                    port: record.port,
                    algorithm: record.algorithm,
                    fingerprint: fingerprint_sha256(&record.blob),
                })
                .collect()
        })
    }

    /// 拒绝：唤醒全部等待者（其连接以 HostKeyRejected 失败），返回 challenge 供上层关闭 Session。
    pub fn reject(&self, challenge_id: &str) -> Result<HostIdentityChallenge, AppError> {
        let wait = self.remove_pending(challenge_id)?;
        let challenge = wait.challenge.clone();
        Self::decide(&wait, Decision::Rejected);
        Ok(challenge)
    }

    /// 会话关闭路径：取消该 Session 的全部等待者并清除临时信任；
    /// 关闭标签、关闭 Session 与应用退出均不得遗留可认证的连接。
    pub fn cancel_session(&self, session_id: &str) {
        let cancelled = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state.trusted.retain(|key| key.session_id != session_id);
            state.cancelled.insert(session_id.to_string());
            let targets: Vec<Arc<ChallengeWait>> = state
                .pending
                .iter()
                .filter(|(_, wait)| wait.challenge.session_id == session_id)
                .map(|(_, wait)| wait.clone())
                .collect();
            for wait in &targets {
                state.pending.remove(&wait.challenge.challenge_id);
                state
                    .pending_index
                    .remove(&IdentityKey::from_challenge(&wait.challenge));
            }
            targets
        };
        for wait in cancelled {
            Self::decide(&wait, Decision::Cancelled);
        }
    }

    /// 取消全部等待者（应用退出路径与测试使用）。
    pub fn cancel_all(&self) {
        let all: Vec<String> = {
            let state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state.pending.keys().cloned().collect()
        };
        for challenge_id in all {
            let _ = self.cancel_by_id(&challenge_id);
        }
    }

    /// 移除 pending challenge 并返回等待者句柄；不存在则报错。
    fn remove_pending(&self, challenge_id: &str) -> Result<Arc<ChallengeWait>, AppError> {
        let wait = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let wait = state.pending.remove(challenge_id).ok_or_else(|| {
                AppError::HostKeyChallengeNotFound(challenge_id.to_string().into())
            })?;
            state
                .pending_index
                .remove(&IdentityKey::from_challenge(&wait.challenge));
            wait
        };
        Ok(wait)
    }

    /// 按 challenge_id 取消（cancel_all 复用）；不因已解决而报错。
    fn cancel_by_id(&self, challenge_id: &str) -> Result<(), AppError> {
        if let Ok(wait) = self.remove_pending(challenge_id) {
            Self::decide(&wait, Decision::Cancelled);
        }
        Ok(())
    }

    /// 写入决定并唤醒全部等待者。
    fn decide(wait: &Arc<ChallengeWait>, decision: Decision) {
        let mut guard = wait
            .decision
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *guard = Some(decision);
        wait.signal.notify_all();
    }

    /// 读取指定 Session 当前的 pending challenge（测试与诊断用）。
    #[cfg(test)]
    pub(crate) fn pending_challenge(&self, session_id: &str) -> Option<HostIdentityChallenge> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .pending
            .values()
            .find(|wait| wait.challenge.session_id == session_id)
            .map(|wait| wait.challenge.clone())
    }

    /// 读取指定 challenge 当前合并的等待连接数（测试观察合并进度）。
    #[cfg(test)]
    pub(crate) fn waiting_connections(&self, challenge_id: &str) -> usize {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .pending
            .get(challenge_id)
            .map(|wait| wait.waiting.load(Ordering::Relaxed))
            .unwrap_or(0)
    }

    /// 判断指定 Session 的 endpoint+指纹是否已写入临时信任（测试观察清除行为）。
    #[cfg(test)]
    pub(crate) fn is_trusted(
        &self,
        session_id: &str,
        host: &str,
        port: u16,
        fingerprint: &str,
    ) -> bool {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .trusted
            .contains(&IdentityKey {
                session_id: session_id.to_string(),
                host: host.to_string(),
                port,
                fingerprint: fingerprint.to_string(),
            })
    }
}

#[cfg(test)]
#[path = "host_identity_test.rs"]
mod tests;
