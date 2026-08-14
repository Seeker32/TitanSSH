//! 主机身份确认（TOFU，仅本次接受）：临时信任、pending challenge 与等待者的单一后端权威。
//!
//! 所有 capability（Terminal、SFTP、Monitoring）的 SSH 连接在握手后、认证前
//! 通过 [`HostKeyVerifier`] 进入统一校验；未知主机产生一个 challenge 事件并阻塞
//! 等待用户决定。信任以 Runtime Session 为作用域：同一 Session 内接受过的
//! endpoint+指纹后续连接（含重连）直接放行，Session 关闭即清除。
//! 不把策略复制到各 capability service。

use crate::errors::app_error::AppError;
use crate::models::session::HostIdentityChallenge;
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

/// 主机身份确认服务：临时信任、pending challenge 与等待者的后端权威。
#[derive(Clone)]
pub struct HostIdentityService {
    state: Arc<Mutex<IdentityState>>,
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
        }
    }

    /// 为指定 Runtime Session 构建统一校验闭包，注入 transport / capability 连接路径。
    pub fn verifier<R: Runtime>(&self, app: AppHandle<R>, session_id: String) -> HostKeyVerifier {
        let service = self.clone();
        Arc::new(move |presented| service.verify(&app, &session_id, presented))
    }

    /// 统一校验：已信任直接放行；未知主机派发 challenge 事件并阻塞等待用户决定。
    /// 同一 Session、endpoint 与指纹的并发连接合并到同一 challenge。
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
        let (wait, created) = {
            let mut state = self
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
            match state.pending_index.get(&key) {
                Some(challenge_id) => (
                    state
                        .pending
                        .get(challenge_id)
                        .cloned()
                        .expect("pending_index 与 pending 同步维护"),
                    false,
                ),
                None => {
                    let challenge = HostIdentityChallenge {
                        challenge_id: Uuid::new_v4().to_string(),
                        session_id: session_id.to_string(),
                        host: presented.host.clone(),
                        port: presented.port,
                        key_algorithm: presented.algorithm.clone(),
                        fingerprint: presented.fingerprint.clone(),
                        timestamp: chrono::Utc::now().timestamp_millis(),
                    };
                    let wait = Arc::new(ChallengeWait {
                        decision: Mutex::new(None),
                        signal: Condvar::new(),
                        waiting: AtomicUsize::new(0),
                        challenge,
                    });
                    state
                        .pending
                        .insert(wait.challenge.challenge_id.clone(), wait.clone());
                    state
                        .pending_index
                        .insert(key, wait.challenge.challenge_id.clone());
                    (wait, true)
                }
            }
        };
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
    use serde_json::Value;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::thread;
    use std::time::{Duration, Instant};
    use tauri::Listener;
    use tauri::test::mock_app;

    fn make_presented(fingerprint: &str) -> PresentedHostKey {
        PresentedHostKey {
            host: "10.0.0.8".to_string(),
            port: 22,
            algorithm: "ssh-ed25519".to_string(),
            fingerprint: fingerprint.to_string(),
        }
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

    /// challenge 事件 payload 为 camelCase 且字段完整（前端不解析 SSH key 文本）。
    #[test]
    fn challenge_event_serializes_as_camel_case_payload() {
        let challenge = HostIdentityChallenge {
            challenge_id: "c-1".to_string(),
            session_id: "session-1".to_string(),
            host: "10.0.0.8".to_string(),
            port: 22,
            key_algorithm: "ssh-ed25519".to_string(),
            fingerprint: "SHA256:aaa".to_string(),
            timestamp: 1_710_000_000_000,
        };
        let value = serde_json::to_value(&challenge).unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "challengeId": "c-1",
                "sessionId": "session-1",
                "host": "10.0.0.8",
                "port": 22,
                "keyAlgorithm": "ssh-ed25519",
                "fingerprint": "SHA256:aaa",
                "timestamp": 1_710_000_000_000_i64
            })
        );
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
}
