//! 主机身份确认（TOFU，仅本次接受）：临时信任、pending challenge 与等待者的单一后端权威。
//!
//! 所有 capability（Terminal、SFTP、Monitoring）的 SSH 连接在握手后、认证前
//! 通过 [`HostKeyVerifier`] 进入统一校验；未知主机产生一个 challenge 事件并阻塞
//! 等待用户决定。信任以 Runtime Session 为作用域：同一 Session 内接受过的
//! endpoint+指纹后续连接（含重连）直接放行，Session 关闭即清除。
//! 不把策略复制到各 capability service。

use crate::errors::app_error::AppError;
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
                    session_id.to_string(),
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
                            session_id.to_string(),
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
                    session_id.to_string(),
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
                    Decision::Rejected => Err(AppError::HostKeyRejected(format!(
                        "{}:{} ({})",
                        wait.challenge.host, wait.challenge.port, wait.challenge.fingerprint
                    ))),
                    Decision::Cancelled => Err(AppError::HostKeyVerificationCancelled(
                        session_id.to_string(),
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
            let wait = state
                .pending
                .remove(challenge_id)
                .ok_or_else(|| AppError::HostKeyChallengeNotFound(challenge_id.to_string()))?;
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
            let wait = state
                .pending
                .get(challenge_id)
                .cloned()
                .ok_or_else(|| AppError::HostKeyChallengeNotFound(challenge_id.to_string()))?;

            // 持久化：trust store 内部串行化读写并安全发布，失败不改动旧记录。
            // 写入失败时本 challenge 尚未从 pending 移除，保持未决。
            let store = self
                .trust_store
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone()
                .ok_or_else(|| {
                    AppError::HostKeySaveFailed("信任存储未初始化，无法持久化信任记录".to_string())
                })?;
            store
                .upsert(TrustRecord {
                    host: wait.challenge.host.clone(),
                    port: wait.challenge.port,
                    algorithm: wait.challenge.key_algorithm.clone(),
                    blob: wait.presented_blob.clone(),
                })
                .map_err(|error| AppError::HostKeySaveFailed(error.to_string()))?;

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
                AppError::TrustStoreError("信任存储未初始化，无法清理信任记录".to_string())
            })?;
        store.remove(host, port)
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
            let wait = state
                .pending
                .remove(challenge_id)
                .ok_or_else(|| AppError::HostKeyChallengeNotFound(challenge_id.to_string()))?;
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
mod tests {
    use super::*;
    use crate::storage::trust_store::{TrustRecord, TrustStore};
    use serde_json::Value;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::thread;
    use std::time::{Duration, Instant};
    use tauri::Listener;
    use tauri::test::mock_app;
    use uuid::Uuid;

    fn make_presented(fingerprint: &str) -> PresentedHostKey {
        PresentedHostKey {
            host: "10.0.0.8".to_string(),
            port: 22,
            algorithm: "ssh-ed25519".to_string(),
            fingerprint: fingerprint.to_string(),
            blob: b"blob".to_vec(),
        }
    }

    /// 隔离的临时信任存储路径（默认不存在，等价空信任存储）。
    fn temp_trust_path() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("titan-identity-trust-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).expect("temp dir should be created");
        dir.join("known_hosts")
    }

    /// 构造带信任存储的服务并预置指定记录。
    fn service_with_record(record: TrustRecord) -> (HostIdentityService, PathBuf) {
        let path = temp_trust_path();
        let store = TrustStore::from_file_path(path.clone());
        store.upsert(record).expect("预置记录应写入成功");
        (HostIdentityService::with_trust_store(store), path)
    }

    /// 等待指定 Session 出现 pending challenge（超时则 panic）。
    fn wait_pending(service: &HostIdentityService, session_id: &str) -> HostIdentityChallenge {
        let deadline = Instant::now() + Duration::from_secs(2);
        while service.pending_challenge(session_id).is_none() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        service
            .pending_challenge(session_id)
            .expect("challenge 已创建")
    }

    /// 已保存 key 精确匹配：verify 在认证前直接放行，不产生 challenge。
    #[test]
    fn saved_key_exact_match_skips_challenge() {
        let app = mock_app();
        let events = Arc::new(AtomicUsize::new(0));
        let counter = events.clone();
        app.listen("host-identity:challenge", move |_| {
            counter.fetch_add(1, Ordering::Relaxed);
        });
        let (service, _path) = service_with_record(TrustRecord {
            host: "10.0.0.8".to_string(),
            port: 22,
            algorithm: "ssh-ed25519".to_string(),
            blob: b"blob".to_vec(),
        });
        let verifier = service.verifier(app.handle().clone(), "session-1".to_string());

        verifier(&make_presented("SHA256:match")).expect("匹配记录应静默放行");
        assert!(service.pending_challenge("session-1").is_none());
        assert_eq!(events.load(Ordering::Relaxed), 0, "匹配时不产生 challenge");
    }

    /// 已关闭 Session 的迟到校验器不得借持久化信任继续认证：取消检查先于匹配放行。
    #[test]
    fn cancelled_session_fails_even_when_key_is_saved() {
        let app = mock_app();
        let (service, _path) = service_with_record(TrustRecord {
            host: "10.0.0.8".to_string(),
            port: 22,
            algorithm: "ssh-ed25519".to_string(),
            blob: b"blob".to_vec(),
        });
        let verifier = service.verifier(app.handle().clone(), "session-gone".to_string());

        service.cancel_session("session-gone");
        let error = verifier(&make_presented("SHA256:match")).unwrap_err();
        assert_eq!(
            error.code(),
            "HostKeyVerificationCancelled",
            "关闭后的 Session 即使 key 已保存也不得进入认证"
        );
        assert!(service.pending_challenge("session-gone").is_none());
    }

    /// 已保存 key 的匹配是精确的：host 拼写、端口、算法或公钥任一不同都产生 challenge。
    #[test]
    fn saved_key_match_is_exact_on_all_fields() {
        let app = mock_app();
        let (service, _path) = service_with_record(TrustRecord {
            host: "10.0.0.8".to_string(),
            port: 22,
            algorithm: "ssh-ed25519".to_string(),
            blob: b"blob".to_vec(),
        });
        let verifier = service.verifier(app.handle().clone(), "session-1".to_string());

        let variants = [
            PresentedHostKey {
                host: "10.0.0.9".to_string(),
                ..make_presented("SHA256:a")
            },
            PresentedHostKey {
                port: 2222,
                ..make_presented("SHA256:b")
            },
            PresentedHostKey {
                algorithm: "ssh-rsa".to_string(),
                ..make_presented("SHA256:c")
            },
            PresentedHostKey {
                blob: b"other".to_vec(),
                ..make_presented("SHA256:d")
            },
        ];
        for presented in variants {
            let verifier = verifier.clone();
            let waiter = thread::spawn(move || verifier(&presented));
            let challenge = wait_pending(&service, "session-1");
            service.reject(&challenge.challenge_id).unwrap();
            assert_eq!(
                waiter.join().unwrap().unwrap_err().code(),
                "HostKeyRejected",
                "任一字段不匹配都必须重新确认"
            );
        }
    }

    /// 信任文件缺失：等价空信任存储，未知主机仍走 challenge。
    #[test]
    fn missing_trust_file_means_empty_store() {
        let app = mock_app();
        let service = HostIdentityService::with_trust_store_path(temp_trust_path());
        let verifier = service.verifier(app.handle().clone(), "session-1".to_string());
        let presented = make_presented("SHA256:empty");
        let waiter = thread::spawn(move || verifier(&presented));
        let challenge = wait_pending(&service, "session-1");
        service.accept(&challenge.challenge_id).unwrap();
        waiter.join().unwrap().expect("空信任存储下接受后放行");
    }

    /// 信任文件损坏：fail-closed，verify 以 TrustStoreError 失败，不产生 challenge。
    #[test]
    fn corrupt_trust_store_fails_closed_without_challenge() {
        let app = mock_app();
        let path = temp_trust_path();
        fs::write(&path, "10.0.0.8 ssh-ed25519\n").unwrap();
        let service = HostIdentityService::with_trust_store_path(path);
        let verifier = service.verifier(app.handle().clone(), "session-1".to_string());

        let error = verifier(&make_presented("SHA256:corrupt")).unwrap_err();
        assert_eq!(error.code(), "TrustStoreError");
        assert!(
            service.pending_challenge("session-1").is_none(),
            "fail-closed 不得产生 challenge"
        );
    }

    /// 接受并保存：等待连接继续认证，记录持久化，后续 Session 静默复用。
    #[test]
    fn accept_and_save_persists_and_releases_waiters() {
        let app = mock_app();
        let (service, path) = service_with_record(TrustRecord {
            host: "10.0.0.9".to_string(),
            port: 22,
            algorithm: "ssh-ed25519".to_string(),
            blob: b"unrelated".to_vec(),
        });
        let verifier = service.verifier(app.handle().clone(), "session-1".to_string());
        let presented = make_presented("SHA256:save");
        let waiter = thread::spawn(move || verifier(&presented));
        let challenge = wait_pending(&service, "session-1");

        service.accept_and_save(&challenge.challenge_id).unwrap();
        waiter.join().unwrap().expect("保存成功后放行认证");
        // 磁盘真实内容：endpoint 记录已写入（含原无关记录，不丢其他 endpoint）
        let records = TrustStore::from_file_path(path).reload().unwrap();
        assert_eq!(records.len(), 2);
        let saved = records
            .iter()
            .find(|record| record.host == "10.0.0.8" && record.port == 22)
            .expect("endpoint 记录已保存");
        assert_eq!(saved.algorithm, "ssh-ed25519");
        assert_eq!(saved.blob, b"blob");

        // 后续 Runtime Session：匹配记录静默放行，不再提示
        let verifier_b = service.verifier(app.handle().clone(), "session-b".to_string());
        verifier_b(&make_presented("SHA256:save")).expect("保存后新 Session 不再提示");
    }

    /// 同一 endpoint 已保存不同 key：仍产生 challenge；接受并保存覆盖为当前记录。
    #[test]
    fn accept_and_save_overwrites_previous_record_for_endpoint() {
        let app = mock_app();
        let (service, path) = service_with_record(TrustRecord {
            host: "10.0.0.8".to_string(),
            port: 22,
            algorithm: "ssh-rsa".to_string(),
            blob: b"old-key".to_vec(),
        });
        let verifier = service.verifier(app.handle().clone(), "session-1".to_string());
        let presented = make_presented("SHA256:rotate");
        let waiter = thread::spawn(move || verifier(&presented));
        let challenge = wait_pending(&service, "session-1");

        service.accept_and_save(&challenge.challenge_id).unwrap();
        waiter.join().unwrap().unwrap();
        let records = TrustStore::from_file_path(path).reload().unwrap();
        assert_eq!(records.len(), 1, "同一 endpoint 只保留一条记录");
        assert_eq!(records[0].algorithm, "ssh-ed25519");
        assert_eq!(records[0].blob, b"blob");
    }

    /// 保存失败：challenge 保持未决，不降级为临时信任，错误结构化返回。
    #[test]
    fn save_failure_keeps_challenge_unresolved_without_temporary_trust() {
        let app = mock_app();
        let events = Arc::new(AtomicUsize::new(0));
        let counter = events.clone();
        app.listen("host-identity:challenge", move |_| {
            counter.fetch_add(1, Ordering::Relaxed);
        });
        let (service, path) = service_with_record(TrustRecord {
            host: "10.0.0.9".to_string(),
            port: 22,
            algorithm: "ssh-ed25519".to_string(),
            blob: b"unrelated".to_vec(),
        });
        let verifier = service.verifier(app.handle().clone(), "session-1".to_string());
        let presented = make_presented("SHA256:fail");
        let waiter = thread::spawn({
            let presented = presented.clone();
            let verifier = verifier.clone();
            move || verifier(&presented)
        });
        let challenge = wait_pending(&service, "session-1");

        // 破坏发布目标：文件路径替换为目录，写盘必然失败（读取缓存不受影响）
        fs::remove_file(&path).unwrap();
        fs::create_dir_all(&path).unwrap();
        let error = service
            .accept_and_save(&challenge.challenge_id)
            .unwrap_err();
        assert_eq!(error.code(), "HostKeySaveFailed");
        // challenge 保持未决：等待者仍在等待，pending 未清除
        assert_eq!(
            service.pending_challenge("session-1").unwrap().challenge_id,
            challenge.challenge_id
        );
        // 不降级为临时信任：同一 Session 的并发连接合并到同一 challenge，而非放行
        let verifier2 = service.verifier(app.handle().clone(), "session-1".to_string());
        let presented2 = presented.clone();
        let waiter2 = thread::spawn(move || verifier2(&presented2));
        let deadline = Instant::now() + Duration::from_secs(2);
        while service.waiting_connections(&challenge.challenge_id) < 2 && Instant::now() < deadline
        {
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            service.waiting_connections(&challenge.challenge_id),
            2,
            "保存失败不得授予临时信任"
        );
        assert_eq!(
            events.load(Ordering::Relaxed),
            1,
            "合并到同一 challenge 不重复派发"
        );
        // 用户可改选仅本次接受：challenge 正常解决，全部等待者放行
        service.accept(&challenge.challenge_id).unwrap();
        waiter.join().unwrap().expect("改选仅本次接受后放行");
        waiter2.join().unwrap().expect("改选仅本次接受后放行");
    }

    /// 应用 setup 路径：init_trust_store 后保存记录可读，未初始化则保存失败（fail-closed）。
    #[test]
    fn init_trust_store_populates_managed_store() {
        let app = mock_app();
        let service = HostIdentityService::new();
        service
            .init_trust_store(app.handle())
            .expect("初始化应成功");
        // mock app 的应用数据目录在测试间共享：使用唯一 host 避免测试间写入互相干扰
        let unique_host = format!("10.0.0.{}", &Uuid::new_v4().simple().to_string()[..8]);
        let presented = PresentedHostKey {
            host: unique_host.clone(),
            ..make_presented("SHA256:init")
        };
        let verifier = service.verifier(app.handle().clone(), "session-1".to_string());
        let waiter = thread::spawn(move || verifier(&presented));
        let challenge = wait_pending(&service, "session-1");
        service.accept_and_save(&challenge.challenge_id).unwrap();
        waiter.join().unwrap().unwrap();
        // 初始化后的持久化信任生效：新 Session 同 endpoint 静默放行
        let verifier_b = service.verifier(app.handle().clone(), "session-b".to_string());
        verifier_b(&PresentedHostKey {
            host: unique_host,
            ..make_presented("SHA256:init")
        })
        .expect("初始化后的信任存储应命中");
    }

    /// 未初始化信任存储时保存失败且 challenge 保持未决（fail-closed，不吞错）。
    #[test]
    fn accept_and_save_without_store_keeps_challenge_pending() {
        let app = mock_app();
        let service = HostIdentityService::new();
        let verifier = service.verifier(app.handle().clone(), "session-1".to_string());
        let presented = make_presented("SHA256:nostore");
        let waiter = thread::spawn(move || verifier(&presented));
        let challenge = wait_pending(&service, "session-1");

        let error = service
            .accept_and_save(&challenge.challenge_id)
            .unwrap_err();
        assert_eq!(error.code(), "HostKeySaveFailed");
        assert_eq!(
            service.pending_challenge("session-1").unwrap().challenge_id,
            challenge.challenge_id
        );
        service.accept(&challenge.challenge_id).unwrap();
        waiter.join().unwrap().unwrap();
    }

    /// 跨 Session：保存成功后，其他 Session 中同 endpoint + 同 key 的 pending challenge 一并放行。
    #[test]
    fn save_releases_identical_pending_challenges_in_other_sessions() {
        let app = mock_app();
        let (service, _path) = service_with_record(TrustRecord {
            host: "10.0.0.9".to_string(),
            port: 22,
            algorithm: "ssh-ed25519".to_string(),
            blob: b"unrelated".to_vec(),
        });
        let verifier_a = service.verifier(app.handle().clone(), "session-a".to_string());
        let verifier_b = service.verifier(app.handle().clone(), "session-b".to_string());
        let presented = make_presented("SHA256:shared");
        let waiter_a = thread::spawn({
            let presented = presented.clone();
            let verifier_a = verifier_a.clone();
            move || verifier_a(&presented)
        });
        let waiter_b = thread::spawn(move || verifier_b(&presented));
        let challenge_a = wait_pending(&service, "session-a");
        let challenge_b = wait_pending(&service, "session-b");
        assert_ne!(challenge_a.challenge_id, challenge_b.challenge_id);

        service.accept_and_save(&challenge_a.challenge_id).unwrap();
        waiter_a.join().unwrap().expect("发起保存的 Session 放行");
        waiter_b
            .join()
            .unwrap()
            .expect("相同 endpoint+key 的其他 Session 一并放行");
        assert!(service.pending_challenge("session-b").is_none());
    }

    /// 跨 Session 保存不放行不同 key 的 pending challenge；其等待者仍按需解决。
    #[test]
    fn save_does_not_release_pending_challenge_with_different_key() {
        let app = mock_app();
        let (service, _path) = service_with_record(TrustRecord {
            host: "10.0.0.9".to_string(),
            port: 22,
            algorithm: "ssh-ed25519".to_string(),
            blob: b"unrelated".to_vec(),
        });
        let verifier_a = service.verifier(app.handle().clone(), "session-a".to_string());
        let verifier_c = service.verifier(app.handle().clone(), "session-c".to_string());
        let presented_a = make_presented("SHA256:saved-key");
        let presented_c = PresentedHostKey {
            blob: b"different".to_vec(),
            ..make_presented("SHA256:other-key")
        };
        let waiter_a = thread::spawn(move || verifier_a(&presented_a));
        let waiter_c = thread::spawn(move || verifier_c(&presented_c));
        let challenge_a = wait_pending(&service, "session-a");
        let challenge_c = wait_pending(&service, "session-c");

        service.accept_and_save(&challenge_a.challenge_id).unwrap();
        waiter_a.join().unwrap().unwrap();
        // session-c 的 key 不同：保存不自动放行，challenge 仍待用户决定
        assert_eq!(
            service.pending_challenge("session-c").unwrap().challenge_id,
            challenge_c.challenge_id
        );
        service.accept(&challenge_c.challenge_id).unwrap();
        waiter_c.join().unwrap().unwrap();
    }

    /// 首次未知主机产生 challenge 事件；接受后同一 Session 的后续连接（含重连）直接放行。
    #[test]
    fn accept_once_allows_subsequent_connections_in_same_session() {
        let app = mock_app();
        let service = HostIdentityService::new();
        let events = Arc::new(AtomicUsize::new(0));
        let counter = events.clone();
        app.listen("host-identity:challenge", move |_| {
            counter.fetch_add(1, Ordering::Relaxed);
        });

        let verifier = service.verifier(app.handle().clone(), "session-1".to_string());
        let presented = make_presented("SHA256:aaa");

        // 首次：阻塞等待用户决定
        let wait_verifier = verifier.clone();
        let presented_clone = presented.clone();
        let waiter = thread::spawn(move || wait_verifier(&presented_clone));
        let deadline = Instant::now() + Duration::from_secs(2);
        while service.pending_challenge("session-1").is_none() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        let challenge = service
            .pending_challenge("session-1")
            .expect("challenge 已创建");

        service.accept(&challenge.challenge_id).unwrap();
        waiter.join().unwrap().expect("接受后放行");

        // 第二次连接（模拟 capability reconnect）：已信任，直接放行且不产生新 challenge
        verifier(&presented).expect("同一 Session 内已信任");
        assert_eq!(events.load(Ordering::Relaxed), 1);
    }

    /// 信任以 Runtime Session 为作用域：其他 Session 连接同一 endpoint 仍需确认。
    #[test]
    fn trust_is_scoped_to_runtime_session() {
        let app = mock_app();
        let service = HostIdentityService::new();
        let verifier_a = service.verifier(app.handle().clone(), "session-a".to_string());
        let presented = make_presented("SHA256:aaa");

        // session-a 后台等待确认
        let v = verifier_a.clone();
        let p = presented.clone();
        let waiter = thread::spawn(move || v(&p));
        let deadline = Instant::now() + Duration::from_secs(2);
        while service.pending_challenge("session-a").is_none() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        let challenge = service.pending_challenge("session-a").unwrap();
        service.accept(&challenge.challenge_id).unwrap();
        waiter.join().unwrap().unwrap();

        // session-b 连接同一 endpoint+指纹：不受 session-a 的信任影响，产生新 challenge
        let verifier_b = service.verifier(app.handle().clone(), "session-b".to_string());
        let v = verifier_b.clone();
        let p = presented.clone();
        let waiter_b = thread::spawn(move || v(&p));
        let deadline = Instant::now() + Duration::from_secs(2);
        while service.pending_challenge("session-b").is_none() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        let challenge_b = service
            .pending_challenge("session-b")
            .expect("session-b 独立确认");
        service.reject(&challenge_b.challenge_id).unwrap();
        let error = waiter_b.join().unwrap().unwrap_err();
        assert_eq!(error.code(), "HostKeyRejected");
    }

    /// 同一 Session、endpoint 与指纹的并发连接合并为一个 challenge；接受后全部放行。
    #[test]
    fn concurrent_connections_merge_into_single_challenge() {
        let app = mock_app();
        let service = HostIdentityService::new();
        let events = Arc::new(AtomicUsize::new(0));
        let counter = events.clone();
        app.listen("host-identity:challenge", move |_| {
            counter.fetch_add(1, Ordering::Relaxed);
        });

        let verifier = service.verifier(app.handle().clone(), "session-1".to_string());
        let presented = make_presented("SHA256:merge");

        let waiters: Vec<_> = (0..4)
            .map(|_| {
                let v = verifier.clone();
                let p = presented.clone();
                thread::spawn(move || v(&p))
            })
            .collect();

        // 等待 challenge 出现；多个并发等待者只产生一个 challenge
        let deadline = Instant::now() + Duration::from_secs(2);
        while service.pending_challenge("session-1").is_none() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        thread::sleep(Duration::from_millis(50));
        let challenge = service
            .pending_challenge("session-1")
            .expect("challenge 已创建");

        service.accept(&challenge.challenge_id).unwrap();
        for waiter in waiters {
            waiter.join().unwrap().expect("全部等待者接受后继续");
        }
        assert_eq!(
            events.load(Ordering::Relaxed),
            1,
            "并发连接合并为一个 challenge"
        );
    }

    /// 拒绝后全部等待者以 HostKeyRejected 失败，不进入认证。
    #[test]
    fn reject_fails_all_waiters() {
        let app = mock_app();
        let service = HostIdentityService::new();
        let verifier = service.verifier(app.handle().clone(), "session-1".to_string());
        let presented = make_presented("SHA256:deny");

        let waiters: Vec<_> = (0..3)
            .map(|_| {
                let v = verifier.clone();
                let p = presented.clone();
                thread::spawn(move || v(&p))
            })
            .collect();
        let deadline = Instant::now() + Duration::from_secs(2);
        while service.pending_challenge("session-1").is_none() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        let challenge = service.pending_challenge("session-1").unwrap();
        // 等待全部并发连接合并到同一 challenge 后再拒绝，避免迟到连接另建 challenge
        while service.waiting_connections(&challenge.challenge_id) < 3 {
            assert!(
                Instant::now() < deadline,
                "并发连接应在超时前合并到同一 challenge"
            );
            thread::sleep(Duration::from_millis(10));
        }

        let rejected = service.reject(&challenge.challenge_id).unwrap();
        assert_eq!(rejected.session_id, "session-1");
        for waiter in waiters {
            let error = waiter.join().unwrap().unwrap_err();
            assert_eq!(error.code(), "HostKeyRejected");
        }
        // 拒绝不写入信任：新连接产生新 challenge 而非静默放行
        let v = verifier.clone();
        let p = presented.clone();
        let retry = thread::spawn(move || v(&p));
        let deadline = Instant::now() + Duration::from_secs(2);
        while service.pending_challenge("session-1").is_none() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        let retried = service
            .pending_challenge("session-1")
            .expect("拒绝后新连接重新确认");
        service.reject(&retried.challenge_id).unwrap();
        assert_eq!(retry.join().unwrap().unwrap_err().code(), "HostKeyRejected");
    }

    /// 会话关闭取消全部等待者并清除临时信任；等待者以取消错误退出且不再阻塞。
    #[test]
    fn cancel_session_waits_no_more_and_clears_trust() {
        let app = mock_app();
        let service = HostIdentityService::new();
        let verifier = service.verifier(app.handle().clone(), "session-1".to_string());
        let presented = make_presented("SHA256:cancel");

        let v = verifier.clone();
        let p = presented.clone();
        let waiter = thread::spawn(move || v(&p));
        let deadline = Instant::now() + Duration::from_secs(2);
        while service.pending_challenge("session-1").is_none() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }

        service.cancel_session("session-1");
        let error = waiter.join().unwrap().unwrap_err();
        assert_eq!(error.code(), "HostKeyVerificationCancelled");
        assert!(service.pending_challenge("session-1").is_none());

        // 清除临时信任：另一 Session 先接受后取消，信任必须被移除（直接观察状态，
        // 因为取消后的 Session 校验器按设计直接失败，不会再产生 challenge）
        let verifier_b = service.verifier(app.handle().clone(), "session-2".to_string());
        let v = verifier_b.clone();
        let p = presented.clone();
        let waiter = thread::spawn(move || v(&p));
        let deadline = Instant::now() + Duration::from_secs(2);
        while service.pending_challenge("session-2").is_none() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        let challenge = service
            .pending_challenge("session-2")
            .expect("session-2 产生 challenge");
        service.accept(&challenge.challenge_id).unwrap();
        waiter.join().unwrap().expect("接受后放行");
        assert!(
            service.is_trusted("session-2", "10.0.0.8", 22, "SHA256:cancel"),
            "接受后写入临时信任"
        );
        service.cancel_session("session-2");
        assert!(
            !service.is_trusted("session-2", "10.0.0.8", 22, "SHA256:cancel"),
            "Session 关闭必须清除临时信任"
        );
    }

    /// 关闭后的 Session 上迟到到达的校验器（如已发放给 Monitoring worker）不得再
    /// 创建无人取消的 challenge：verify 立即以取消错误返回，等待者不会永久阻塞。
    #[test]
    fn cancelled_session_verifier_fails_fast_without_new_challenge() {
        let app = mock_app();
        let service = HostIdentityService::new();
        let verifier = service.verifier(app.handle().clone(), "session-gone".to_string());

        service.cancel_session("session-gone");
        let error = verifier(&make_presented("SHA256:late")).unwrap_err();
        assert_eq!(error.code(), "HostKeyVerificationCancelled");
        assert!(
            service.pending_challenge("session-gone").is_none(),
            "取消后的 Session 不得产生新的 pending challenge"
        );
    }

    /// 应用退出路径：cancel_all 唤醒全部 Session 的全部等待者，pending 清空。
    #[test]
    fn cancel_all_wakes_all_waiters() {
        let app = mock_app();
        let service = HostIdentityService::new();
        let verifier_a = service.verifier(app.handle().clone(), "session-a".to_string());
        let verifier_b = service.verifier(app.handle().clone(), "session-b".to_string());

        let waiters: Vec<_> = [("session-a", &verifier_a), ("session-b", &verifier_b)]
            .iter()
            .map(|(session_id, verifier)| {
                let v = (*verifier).clone();
                let p = make_presented(&format!("SHA256:exit-{session_id}"));
                thread::spawn(move || v(&p))
            })
            .collect();
        let deadline = Instant::now() + Duration::from_secs(2);
        while (service.pending_challenge("session-a").is_none()
            || service.pending_challenge("session-b").is_none())
            && Instant::now() < deadline
        {
            thread::sleep(Duration::from_millis(10));
        }

        service.cancel_all();
        for waiter in waiters {
            let error = waiter.join().unwrap().unwrap_err();
            assert_eq!(error.code(), "HostKeyVerificationCancelled");
        }
        assert!(service.pending_challenge("session-a").is_none());
        assert!(service.pending_challenge("session-b").is_none());
    }

    /// accept/reject 不存在的 challenge 返回稳定错误。
    #[test]
    fn unknown_challenge_returns_stable_error() {
        let service = HostIdentityService::new();
        assert_eq!(
            service.accept("missing").unwrap_err().code(),
            "HostKeyChallengeNotFound"
        );
        assert_eq!(
            service.reject("missing").unwrap_err().code(),
            "HostKeyChallengeNotFound"
        );
    }

    /// challenge 事件 payload 为 camelCase 且字段完整（前端不解析 SSH key 文本）；
    /// Unknown 与 Changed 的 kind、stored 字段均正确序列化。
    #[test]
    fn challenge_event_serializes_as_camel_case_payload() {
        let unknown = HostIdentityChallenge {
            challenge_id: "c-1".to_string(),
            session_id: "session-1".to_string(),
            host: "10.0.0.8".to_string(),
            port: 22,
            kind: HostIdentityChallengeKind::Unknown,
            key_algorithm: "ssh-ed25519".to_string(),
            fingerprint: "SHA256:aaa".to_string(),
            stored_algorithm: None,
            stored_fingerprint: None,
            timestamp: 1_710_000_000_000,
        };
        let value = serde_json::to_value(&unknown).unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "challengeId": "c-1",
                "sessionId": "session-1",
                "host": "10.0.0.8",
                "port": 22,
                "kind": "Unknown",
                "keyAlgorithm": "ssh-ed25519",
                "fingerprint": "SHA256:aaa",
                "storedAlgorithm": null,
                "storedFingerprint": null,
                "timestamp": 1_710_000_000_000_i64
            })
        );
        let changed = HostIdentityChallenge {
            challenge_id: "c-2".to_string(),
            kind: HostIdentityChallengeKind::Changed,
            key_algorithm: "ssh-rsa".to_string(),
            fingerprint: "SHA256:bbb".to_string(),
            stored_algorithm: Some("ssh-ed25519".to_string()),
            stored_fingerprint: Some(fingerprint_sha256(b"old")),
            ..unknown
        };
        let value = serde_json::to_value(&changed).unwrap();
        assert_eq!(value["kind"], "Changed");
        assert_eq!(value["storedAlgorithm"], "ssh-ed25519");
        assert_eq!(value["storedFingerprint"], fingerprint_sha256(b"old"));
        let _: Value = value;
    }

    /// 指纹使用 OpenSSH 已知向量：SHA-256("abc") 的 base64 无填充。
    #[test]
    fn fingerprint_matches_known_vector() {
        assert_eq!(
            fingerprint_sha256(b"abc"),
            "SHA256:ungWv48Bz+pBQUDeXa4iI7ADYaOWF3qctBD/YfIAFa0"
        );
    }

    /// 主机密钥算法名映射为 OpenSSH 风格。
    #[test]
    fn algorithm_names_follow_openssh_style() {
        assert_eq!(algorithm_name(HostKeyType::Ed25519), "ssh-ed25519");
        assert_eq!(algorithm_name(HostKeyType::Rsa), "ssh-rsa");
        assert_eq!(algorithm_name(HostKeyType::Ecdsa256), "ecdsa-sha2-nistp256");
        assert_eq!(algorithm_name(HostKeyType::Unknown), "unknown");
    }

    /// 生命周期清理移除持久化记录后，新 Session 将 endpoint 视为未知并重新确认。
    #[test]
    fn forget_endpoint_removes_persisted_record_and_new_sessions_prompt() {
        let app = mock_app();
        let (service, path) = service_with_record(TrustRecord {
            host: "10.0.0.8".to_string(),
            port: 22,
            algorithm: "ssh-ed25519".to_string(),
            blob: b"blob".to_vec(),
        });

        service.forget_endpoint("10.0.0.8", 22).unwrap();
        // 持久化记录已删除（重新构造 TrustStore 绕过内存缓存观察磁盘真相）
        assert_eq!(
            TrustStore::from_file_path(path)
                .lookup("10.0.0.8", 22)
                .unwrap(),
            None
        );
        // 新 Session 重新视为未知：产生 challenge 等待确认
        let verifier = service.verifier(app.handle().clone(), "session-1".to_string());
        let presented = make_presented("SHA256:again");
        let waiter = thread::spawn(move || verifier(&presented));
        let challenge = wait_pending(&service, "session-1");
        service.reject(&challenge.challenge_id).unwrap();
        assert_eq!(
            waiter.join().unwrap().unwrap_err().code(),
            "HostKeyRejected"
        );
    }

    /// 清理不干扰运行中的 Runtime Session：已持有的临时信任持续到 Session 关闭，
    /// 持久化记录移除后同 Session 重连仍放行。
    #[test]
    fn forget_endpoint_keeps_active_session_temporary_trust() {
        let app = mock_app();
        let (service, _path) = service_with_record(TrustRecord {
            host: "10.0.0.8".to_string(),
            port: 22,
            algorithm: "ssh-ed25519".to_string(),
            blob: b"persisted".to_vec(),
        });
        // session-1 呈现与持久化不同的 key：challenge → 仅本次接受（临时信任）
        let presented = PresentedHostKey {
            blob: b"rotated".to_vec(),
            ..make_presented("SHA256:rotated")
        };
        let verifier = service.verifier(app.handle().clone(), "session-1".to_string());
        let waiter = thread::spawn({
            let verifier = verifier.clone();
            let presented = presented.clone();
            move || verifier(&presented)
        });
        let challenge = wait_pending(&service, "session-1");
        service.accept(&challenge.challenge_id).unwrap();
        waiter.join().unwrap().unwrap();

        // 生命周期清理移除持久化记录：不影响运行中的 Session
        service.forget_endpoint("10.0.0.8", 22).unwrap();

        // 同 Session 重连仍放行（临时信任持续到关闭）
        verifier(&presented).expect("活动 Session 的临时信任不受清理影响");
        // 新 Session 视为未知：重新触发确认
        let verifier_b = service.verifier(app.handle().clone(), "session-b".to_string());
        let waiter_b = thread::spawn({
            let verifier = verifier_b.clone();
            let presented = presented.clone();
            move || verifier(&presented)
        });
        let challenge_b = wait_pending(&service, "session-b");
        service.reject(&challenge_b.challenge_id).unwrap();
        assert_eq!(
            waiter_b.join().unwrap().unwrap_err().code(),
            "HostKeyRejected"
        );
    }

    /// 已通过持久化匹配静默验证的 Session：清理后同 Session 重连仍静默放行
    /// （已验证决定持续到 Session 关闭），新 Session 重新确认。
    #[test]
    fn forget_endpoint_keeps_silently_verified_session_decision() {
        let app = mock_app();
        let (service, _path) = service_with_record(TrustRecord {
            host: "10.0.0.8".to_string(),
            port: 22,
            algorithm: "ssh-ed25519".to_string(),
            blob: b"blob".to_vec(),
        });
        let verifier = service.verifier(app.handle().clone(), "session-1".to_string());
        let presented = make_presented("SHA256:match");
        verifier(&presented).expect("持久化匹配静默放行");

        // 生命周期清理移除持久化记录
        service.forget_endpoint("10.0.0.8", 22).unwrap();
        // 同 Session 重连：已验证决定持续到 Session 关闭，不产生新 challenge
        verifier(&presented).expect("已验证决定持续到 Session 关闭");
        assert!(service.pending_challenge("session-1").is_none());
        // 新 Session 视为未知并重新确认
        let verifier_b = service.verifier(app.handle().clone(), "session-b".to_string());
        let waiter_b = thread::spawn(move || verifier_b(&presented));
        let challenge_b = wait_pending(&service, "session-b");
        service.reject(&challenge_b.challenge_id).unwrap();
        assert_eq!(
            waiter_b.join().unwrap().unwrap_err().code(),
            "HostKeyRejected"
        );
    }

    /// 移除不存在的 endpoint 幂等成功（HostConfig 从未受信任的 endpoint 编辑路径）。
    #[test]
    fn forget_endpoint_missing_endpoint_is_idempotent() {
        let (service, _path) = service_with_record(TrustRecord {
            host: "10.0.0.8".to_string(),
            port: 22,
            algorithm: "ssh-ed25519".to_string(),
            blob: b"blob".to_vec(),
        });
        service.forget_endpoint("10.0.0.9", 2222).unwrap();
        service.forget_endpoint("10.0.0.8", 22).unwrap();
    }

    /// 信任存储未初始化时清理 fail-closed：显式报错，不得静默吞掉未完成的清理。
    #[test]
    fn forget_endpoint_without_store_fails_closed() {
        let service = HostIdentityService::new();
        let error = service.forget_endpoint("10.0.0.8", 22).unwrap_err();
        assert_eq!(error.code(), "TrustStoreError");
    }

    /// 等待指定 Session 出现与给定 id 不同的 pending challenge（旧 challenge 被取代后
    /// 新 challenge 出现）；超时则 panic。
    fn wait_pending_other(
        service: &HostIdentityService,
        session_id: &str,
        exclude: &str,
    ) -> HostIdentityChallenge {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if let Some(challenge) = service.pending_challenge(session_id)
                && challenge.challenge_id != exclude
            {
                return challenge;
            }
            assert!(Instant::now() < deadline, "新 challenge 应在超时前创建");
            thread::sleep(Duration::from_millis(10));
        }
    }

    /// 已保存 key 与呈现不一致：产生 Changed challenge，携带旧记录与新呈现的算法/指纹，
    /// 不覆盖或删除旧记录，也不开始认证；替换成功后 endpoint 只保留呈现 key。
    #[test]
    fn changed_key_produces_changed_challenge_with_stored_and_presented_info() {
        let app = mock_app();
        let payloads = Arc::new(Mutex::new(Vec::new()));
        let captured = payloads.clone();
        app.listen("host-identity:challenge", move |event| {
            let payload: HostIdentityChallenge =
                serde_json::from_str(event.payload()).expect("payload 可反序列化");
            captured.lock().unwrap().push(payload);
        });
        let (service, path) = service_with_record(TrustRecord {
            host: "10.0.0.8".to_string(),
            port: 22,
            algorithm: "ssh-ed25519".to_string(),
            blob: b"old-blob".to_vec(),
        });
        let verifier = service.verifier(app.handle().clone(), "session-1".to_string());
        let presented = PresentedHostKey {
            algorithm: "ssh-rsa".to_string(),
            blob: b"new-blob".to_vec(),
            ..make_presented("SHA256:presented")
        };
        let waiter = thread::spawn({
            let verifier = verifier.clone();
            let presented = presented.clone();
            move || verifier(&presented)
        });
        let challenge = wait_pending(&service, "session-1");

        // Changed challenge 同时展示旧记录与新呈现的算法/指纹
        assert_eq!(challenge.kind, HostIdentityChallengeKind::Changed);
        assert_eq!(challenge.key_algorithm, "ssh-rsa");
        assert_eq!(challenge.fingerprint, "SHA256:presented");
        assert_eq!(challenge.stored_algorithm.as_deref(), Some("ssh-ed25519"));
        assert_eq!(
            challenge.stored_fingerprint.as_deref(),
            Some(fingerprint_sha256(b"old-blob").as_str())
        );
        // 事件 payload 同样携带 kind 与新旧信息
        let payloads = payloads.lock().unwrap();
        assert_eq!(payloads.len(), 1);
        assert_eq!(payloads[0].kind, HostIdentityChallengeKind::Changed);
        assert_eq!(payloads[0].stored_algorithm.as_deref(), Some("ssh-ed25519"));
        drop(payloads);

        // 旧记录未被覆盖或删除（磁盘真相），认证未开始（等待者仍在等待）
        let records = TrustStore::from_file_path(path.clone()).reload().unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].algorithm, "ssh-ed25519");
        assert_eq!(records[0].blob, b"old-blob");
        assert_eq!(service.waiting_connections(&challenge.challenge_id), 1);

        // 替换成功：endpoint 只保留呈现 key
        service.accept_and_save(&challenge.challenge_id).unwrap();
        waiter.join().unwrap().expect("替换成功后放行认证");
        let records = TrustStore::from_file_path(path).reload().unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].algorithm, "ssh-rsa");
        assert_eq!(records[0].blob, b"new-blob");
    }

    /// 仅算法变化（公钥材料相同）同样产生 Changed challenge，不静默放行。
    #[test]
    fn algorithm_change_alone_produces_changed_challenge() {
        let app = mock_app();
        let (service, _path) = service_with_record(TrustRecord {
            host: "10.0.0.8".to_string(),
            port: 22,
            algorithm: "ssh-rsa".to_string(),
            blob: b"same-blob".to_vec(),
        });
        let verifier = service.verifier(app.handle().clone(), "session-1".to_string());
        let presented = PresentedHostKey {
            blob: b"same-blob".to_vec(),
            ..make_presented("SHA256:same")
        };
        let waiter = thread::spawn(move || verifier(&presented));
        let challenge = wait_pending(&service, "session-1");
        assert_eq!(challenge.kind, HostIdentityChallengeKind::Changed);
        assert_eq!(challenge.stored_algorithm.as_deref(), Some("ssh-rsa"));
        service.reject(&challenge.challenge_id).unwrap();
        assert_eq!(
            waiter.join().unwrap().unwrap_err().code(),
            "HostKeyRejected"
        );
    }

    /// 仅本次接受以 Runtime Session 为作用域：Changed challenge 的临时接受只放行当前
    /// Session（含重连），其他 Session 相同 endpoint + 相同呈现 key 仍独立等待。
    #[test]
    fn changed_challenge_accept_once_is_scoped_to_current_session() {
        let app = mock_app();
        let (service, _path) = service_with_record(TrustRecord {
            host: "10.0.0.8".to_string(),
            port: 22,
            algorithm: "ssh-ed25519".to_string(),
            blob: b"stored".to_vec(),
        });
        let presented = PresentedHostKey {
            blob: b"rotated".to_vec(),
            ..make_presented("SHA256:rotated")
        };
        let verifier_a = service.verifier(app.handle().clone(), "session-a".to_string());
        let verifier_b = service.verifier(app.handle().clone(), "session-b".to_string());
        let waiter_a = thread::spawn({
            let verifier = verifier_a.clone();
            let presented = presented.clone();
            move || verifier(&presented)
        });
        let waiter_b = thread::spawn({
            let verifier = verifier_b.clone();
            let presented = presented.clone();
            move || verifier(&presented)
        });
        let challenge_a = wait_pending(&service, "session-a");
        let challenge_b = wait_pending(&service, "session-b");
        assert_eq!(challenge_a.kind, HostIdentityChallengeKind::Changed);
        assert_eq!(challenge_b.kind, HostIdentityChallengeKind::Changed);
        assert_ne!(challenge_a.challenge_id, challenge_b.challenge_id);

        // 仅本次接受只解决 session-a 的 challenge
        service.accept(&challenge_a.challenge_id).unwrap();
        waiter_a.join().unwrap().expect("session-a 放行");
        assert_eq!(
            service.pending_challenge("session-b").unwrap().challenge_id,
            challenge_b.challenge_id,
            "其他 Session 仍独立等待"
        );
        // 同 Session 重连复用临时信任，不产生新 challenge
        verifier_a(&presented).expect("session-a 重连放行");
        // session-b 按自己的决定解决
        service.accept(&challenge_b.challenge_id).unwrap();
        waiter_b.join().unwrap().expect("session-b 按自身决定放行");
    }

    /// 替换成功只自动解决兼容的 pending challenge：相同 endpoint + 相同呈现 key 的
    /// 其他 Session 一并放行；不同呈现 key 或不同 endpoint 不受影响。
    #[test]
    fn replace_releases_only_compatible_changed_challenges() {
        let app = mock_app();
        let (service, path) = service_with_record(TrustRecord {
            host: "10.0.0.8".to_string(),
            port: 22,
            algorithm: "ssh-ed25519".to_string(),
            blob: b"stored".to_vec(),
        });
        let presented = PresentedHostKey {
            blob: b"rotated".to_vec(),
            ..make_presented("SHA256:rotated")
        };
        let presented_other_key = PresentedHostKey {
            blob: b"other-key".to_vec(),
            ..make_presented("SHA256:other")
        };
        let presented_other_endpoint = PresentedHostKey {
            host: "10.0.0.9".to_string(),
            ..presented.clone()
        };
        let verifier_a = service.verifier(app.handle().clone(), "session-a".to_string());
        let verifier_b = service.verifier(app.handle().clone(), "session-b".to_string());
        let verifier_c = service.verifier(app.handle().clone(), "session-c".to_string());
        let verifier_d = service.verifier(app.handle().clone(), "session-d".to_string());
        let waiter_a = thread::spawn({
            let verifier = verifier_a.clone();
            let presented = presented.clone();
            move || verifier(&presented)
        });
        let waiter_b = thread::spawn({
            let verifier = verifier_b.clone();
            let presented = presented.clone();
            move || verifier(&presented)
        });
        let waiter_c = thread::spawn({
            let verifier = verifier_c.clone();
            let presented = presented_other_key.clone();
            move || verifier(&presented)
        });
        let waiter_d = thread::spawn({
            let verifier = verifier_d.clone();
            let presented = presented_other_endpoint.clone();
            move || verifier(&presented)
        });
        let challenge_a = wait_pending(&service, "session-a");
        let challenge_b = wait_pending(&service, "session-b");
        assert_eq!(challenge_b.kind, HostIdentityChallengeKind::Changed);
        let challenge_c = wait_pending(&service, "session-c");
        let challenge_d = wait_pending(&service, "session-d");

        // session-a 替换：同 endpoint + 同呈现 key 的 session-b 一并放行
        service.accept_and_save(&challenge_a.challenge_id).unwrap();
        waiter_a.join().unwrap().expect("发起替换的 Session 放行");
        waiter_b.join().unwrap().expect("兼容 challenge 一并放行");
        // 不同呈现 key / 不同 endpoint 的 challenge 不受影响，仍待各自决定
        assert_eq!(
            service.pending_challenge("session-c").unwrap().challenge_id,
            challenge_c.challenge_id
        );
        assert_eq!(
            service.pending_challenge("session-d").unwrap().challenge_id,
            challenge_d.challenge_id
        );
        // 磁盘只保留呈现 key，且不新增其他 endpoint 记录
        let records = TrustStore::from_file_path(path).reload().unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].host, "10.0.0.8");
        assert_eq!(records[0].blob, b"rotated");
        // 清理剩余挑战
        service.reject(&challenge_c.challenge_id).unwrap();
        service.reject(&challenge_d.challenge_id).unwrap();
        waiter_c.join().unwrap().unwrap_err();
        waiter_d.join().unwrap().unwrap_err();
    }

    /// 替换写入失败：旧信任记录与 pending challenge 均保留，用户可明确改选
    /// 仅本次接受或拒绝；绝不静默降级或丢失旧记录。
    #[test]
    fn replace_write_failure_keeps_old_record_and_pending_challenge() {
        let app = mock_app();
        let (service, path) = service_with_record(TrustRecord {
            host: "10.0.0.8".to_string(),
            port: 22,
            algorithm: "ssh-ed25519".to_string(),
            blob: b"old-blob".to_vec(),
        });
        let verifier = service.verifier(app.handle().clone(), "session-1".to_string());
        let presented = PresentedHostKey {
            blob: b"new-blob".to_vec(),
            ..make_presented("SHA256:new")
        };
        let waiter = thread::spawn({
            let verifier = verifier.clone();
            let presented = presented.clone();
            move || verifier(&presented)
        });
        let challenge = wait_pending(&service, "session-1");
        assert_eq!(challenge.kind, HostIdentityChallengeKind::Changed);

        // 破坏发布目标：文件路径替换为目录，写盘必然失败（读取缓存不受影响）
        fs::remove_file(&path).unwrap();
        fs::create_dir_all(&path).unwrap();
        let error = service
            .accept_and_save(&challenge.challenge_id)
            .unwrap_err();
        assert_eq!(error.code(), "HostKeySaveFailed");
        // 旧信任记录未被失败替换污染：新 Session 呈现旧 key 仍精确匹配静默放行
        let verifier_old = service.verifier(app.handle().clone(), "session-old".to_string());
        verifier_old(&PresentedHostKey {
            blob: b"old-blob".to_vec(),
            ..make_presented("SHA256:old")
        })
        .expect("失败替换不得污染旧信任记录");
        // challenge 保持未决
        assert_eq!(
            service.pending_challenge("session-1").unwrap().challenge_id,
            challenge.challenge_id
        );
        // 用户明确改选仅本次接受：正常解决
        service.accept(&challenge.challenge_id).unwrap();
        waiter.join().unwrap().expect("改选仅本次接受后放行");
    }

    /// challenge 之后服务端再次更换 key：同一 Session、同一 endpoint 的新呈现 key
    /// 取代旧 challenge；旧等待者取消（不得以旧 key 认证），对旧 challenge 的
    /// 接受/替换/拒绝决定一律安全失败，新 challenge 正常可决。
    #[test]
    fn server_rotating_key_after_challenge_supersedes_old_challenge() {
        let app = mock_app();
        let events = Arc::new(AtomicUsize::new(0));
        let counter = events.clone();
        app.listen("host-identity:challenge", move |_| {
            counter.fetch_add(1, Ordering::Relaxed);
        });
        let (service, _path) = service_with_record(TrustRecord {
            host: "10.0.0.8".to_string(),
            port: 22,
            algorithm: "ssh-ed25519".to_string(),
            blob: b"stored".to_vec(),
        });
        let verifier = service.verifier(app.handle().clone(), "session-1".to_string());
        let presented_a = PresentedHostKey {
            blob: b"key-a".to_vec(),
            ..make_presented("SHA256:key-a")
        };
        let waiter_a = thread::spawn({
            let verifier = verifier.clone();
            let presented = presented_a.clone();
            move || verifier(&presented)
        });
        let challenge_a = wait_pending(&service, "session-1");

        // 服务端再次更换 key：新连接呈现 key-b
        let presented_b = PresentedHostKey {
            blob: b"key-b".to_vec(),
            ..make_presented("SHA256:key-b")
        };
        let waiter_b = thread::spawn(move || verifier(&presented_b));
        let challenge_b = wait_pending_other(&service, "session-1", &challenge_a.challenge_id);
        assert_ne!(challenge_a.challenge_id, challenge_b.challenge_id);
        assert_eq!(challenge_b.kind, HostIdentityChallengeKind::Changed);
        assert_eq!(challenge_b.fingerprint, "SHA256:key-b");

        // 旧等待者取消：连接不得以未经确认的旧 key 认证
        assert_eq!(
            waiter_a.join().unwrap().unwrap_err().code(),
            "HostKeyVerificationCancelled"
        );
        // 对旧 challenge 的一切决定安全失败（stale / 重复解决 / 未知 challengeId）
        assert_eq!(
            service
                .accept(&challenge_a.challenge_id)
                .unwrap_err()
                .code(),
            "HostKeyChallengeNotFound"
        );
        assert_eq!(
            service
                .accept_and_save(&challenge_a.challenge_id)
                .unwrap_err()
                .code(),
            "HostKeyChallengeNotFound"
        );
        assert_eq!(
            service
                .reject(&challenge_a.challenge_id)
                .unwrap_err()
                .code(),
            "HostKeyChallengeNotFound"
        );
        // 新 challenge 正常可决：替换为新呈现 key
        service.accept_and_save(&challenge_b.challenge_id).unwrap();
        waiter_b.join().unwrap().expect("新 challenge 决定后放行");
        assert_eq!(
            events.load(Ordering::Relaxed),
            2,
            "两次不同 key 各派发一次 challenge 事件"
        );
    }

    /// 一个 Session 的决定不影响其他 Session 的 pending challenge：接受 session-a 的
    /// challenge 后 session-b 仍独立等待（错误 Session 不得解决他人 challenge）。
    #[test]
    fn decision_does_not_resolve_other_sessions_challenge() {
        let app = mock_app();
        let (service, _path) = service_with_record(TrustRecord {
            host: "10.0.0.8".to_string(),
            port: 22,
            algorithm: "ssh-ed25519".to_string(),
            blob: b"stored".to_vec(),
        });
        let verifier_a = service.verifier(app.handle().clone(), "session-a".to_string());
        let verifier_b = service.verifier(app.handle().clone(), "session-b".to_string());
        let presented = PresentedHostKey {
            blob: b"rotated".to_vec(),
            ..make_presented("SHA256:rotated")
        };
        let waiter_a = thread::spawn({
            let verifier = verifier_a.clone();
            let presented = presented.clone();
            move || verifier(&presented)
        });
        let waiter_b = thread::spawn({
            let verifier = verifier_b.clone();
            let presented = presented.clone();
            move || verifier(&presented)
        });
        let challenge_a = wait_pending(&service, "session-a");
        let challenge_b = wait_pending(&service, "session-b");

        service.accept(&challenge_a.challenge_id).unwrap();
        waiter_a.join().unwrap().unwrap();
        assert_eq!(
            service.pending_challenge("session-b").unwrap().challenge_id,
            challenge_b.challenge_id,
            "接受 session-a 不得解决 session-b 的 challenge"
        );
        // 重复/错误 Session 决定安全失败（stale challengeId），不得影响 session-b
        assert_eq!(
            service
                .accept(&challenge_a.challenge_id)
                .unwrap_err()
                .code(),
            "HostKeyChallengeNotFound"
        );
        assert_eq!(
            service
                .reject(&challenge_a.challenge_id)
                .unwrap_err()
                .code(),
            "HostKeyChallengeNotFound"
        );
        assert_eq!(
            service.pending_challenge("session-b").unwrap().challenge_id,
            challenge_b.challenge_id,
            "错误 Session 的决定不得取消或解决他人 challenge"
        );
        service.reject(&challenge_b.challenge_id).unwrap();
        assert_eq!(
            waiter_b.join().unwrap().unwrap_err().code(),
            "HostKeyRejected"
        );
    }

    /// 并发压力：保存/替换与「服务端再次换 key」（取代旧 challenge）竞争。
    /// 无论竞争顺序如何，结局必须安全：保存成功 ⇒ 磁盘只保留呈现 key；
    /// 保存失败（stale）⇒ 写盘前失败、磁盘仍是旧记录；新 challenge 始终可解决。
    #[test]
    fn save_racing_supersede_never_persists_stale_key() {
        let app = mock_app();
        for _ in 0..25 {
            let (service, path) = service_with_record(TrustRecord {
                host: "10.0.0.8".to_string(),
                port: 22,
                algorithm: "ssh-ed25519".to_string(),
                blob: b"old".to_vec(),
            });
            let verifier = service.verifier(app.handle().clone(), "session-1".to_string());
            let presented_a = PresentedHostKey {
                blob: b"key-a".to_vec(),
                ..make_presented("SHA256:key-a")
            };
            let waiter_a = thread::spawn({
                let verifier = verifier.clone();
                let presented = presented_a.clone();
                move || verifier(&presented)
            });
            let challenge_a = wait_pending(&service, "session-1");

            // 并发：保存 challenge_a（替换记录）与呈现新 key（取代旧 challenge）
            let save_handle = {
                let service = service.clone();
                let challenge_id = challenge_a.challenge_id.clone();
                thread::spawn(move || service.accept_and_save(&challenge_id))
            };
            let presented_b = PresentedHostKey {
                blob: b"key-b".to_vec(),
                ..make_presented("SHA256:key-b")
            };
            let waiter_b = thread::spawn({
                let verifier = verifier.clone();
                move || verifier(&presented_b)
            });

            match save_handle.join().unwrap() {
                Ok(()) => {
                    // 保存成功：磁盘只保留呈现 key，challenge_a 等待者被接受
                    let records = TrustStore::from_file_path(path.clone()).reload().unwrap();
                    assert_eq!(records.len(), 1);
                    assert_eq!(records[0].blob, b"key-a");
                    waiter_a
                        .join()
                        .unwrap()
                        .expect("保存成功应放行本挑战等待者");
                }
                Err(error) => {
                    // stale 保存：challenge_a 先被取代，写盘前安全失败，磁盘仍是旧记录
                    assert_eq!(error.code(), "HostKeyChallengeNotFound");
                    let records = TrustStore::from_file_path(path.clone()).reload().unwrap();
                    assert_eq!(records[0].blob, b"old");
                    assert_eq!(
                        waiter_a.join().unwrap().unwrap_err().code(),
                        "HostKeyVerificationCancelled"
                    );
                }
            }
            // 新 challenge（key-b）始终正常可决：接受后放行，无死锁
            let challenge_b = wait_pending_other(&service, "session-1", &challenge_a.challenge_id);
            service.accept(&challenge_b.challenge_id).unwrap();
            waiter_b.join().unwrap().expect("新 challenge 接受后放行");
        }
    }
}
