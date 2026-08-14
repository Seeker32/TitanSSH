//! Session 传输连接池：基础一条按需建立，按需扩展到最多五路真实并发；
//! 额外连接空闲超时回收，等待 checkout 的任务按 Session 内 FIFO 排队，
//! 排队中的等待者可被 cancel_waiter 按入队序号移除并唤醒（返回 Cancelled）。
//!
//! 本模块只承载连接池的调度与回收逻辑：任务状态机、事件与安全发布仍属于
//! sftp_service，池不接触任务 registry 与传输语义。

use crate::core::host_identity::HostKeyVerifier;
use crate::core::sftp_service::{SftpConnector, SftpRole};
use crate::core::ssh_transport::SftpTransport;
use crate::errors::app_error::AppError;
use crate::models::host::HostConfig;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::{Duration, Instant};

/// 每个 Session 最多持有的传输连接数：基础 1 条 + 按需扩展到最多 5 条
pub(crate) const MAX_TRANSFER_CONNECTIONS_PER_SESSION: usize = 5;

/// 额外传输连接连续空闲超过此时长即回收，基础一条（池内最小序号）保留到 Session 关闭
pub(crate) const TRANSFER_IDLE_TIMEOUT: Duration = Duration::from_secs(60);

/// 传输连接池的单调时间源：生产用系统时钟，测试用手工时钟精确推进。
#[derive(Clone)]
pub(crate) enum TransferClock {
    /// 系统单调时钟：后台回收线程按时真实睡眠
    System,
    /// 手工时钟：测试通过 advance 显式推进，回收时机完全确定
    #[cfg(test)]
    Manual(Arc<AtomicU64>),
}

impl TransferClock {
    /// 创建系统时钟。
    pub(crate) fn system() -> Self {
        Self::System
    }

    /// 创建从零开始的手工时钟。
    #[cfg(test)]
    pub(crate) fn manual() -> Self {
        Self::Manual(Arc::new(AtomicU64::new(0)))
    }

    /// 推进手工时钟；系统时钟下为 no-op。
    #[cfg(test)]
    pub(crate) fn advance(&self, millis: u64) {
        if let Self::Manual(elapsed) = self {
            elapsed.fetch_add(millis, Ordering::Relaxed);
        }
    }

    /// 读取当前单调时刻。
    fn now(&self) -> Instant {
        match self {
            Self::System => Instant::now(),
            #[cfg(test)]
            Self::Manual(elapsed) => {
                static MANUAL_EPOCH: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
                *MANUAL_EPOCH.get_or_init(Instant::now)
                    + Duration::from_millis(elapsed.load(Ordering::Relaxed))
            }
        }
    }

    /// 是否使用系统时钟：手工时钟由测试显式驱动回收，不启动后台线程。
    fn is_system(&self) -> bool {
        matches!(self, Self::System)
    }
}

/// 传输连接池：每个 Session 基础保留一条传输连接，按需扩展到最多五条；
/// 超出基础一条的连接连续空闲超过阈值后回收，等待 checkout 的任务按 Session 内 FIFO 排队。
pub(crate) struct TransferPool {
    /// 建连所需主机配置
    host: HostConfig,
    /// 主机身份统一校验器：每条传输连接握手后、认证前生效
    verifier: HostKeyVerifier,
    /// 传输连接 adapter
    connector: SftpConnector,
    /// 单调时间源：决定空闲回收时机
    clock: Arc<TransferClock>,
    /// 额外连接空闲回收阈值
    idle_timeout: Duration,
    /// 池状态：连接槽、FIFO 等待队列、关闭标志与后台回收线程标记
    state: Mutex<PoolState>,
    /// 建连序号发生器：池内每条连接唯一，最小序号为基础连接
    next_seq: AtomicU64,
    /// 指向自身的弱引用：后台回收线程借此保活并访问池
    self_ref: Weak<TransferPool>,
}

/// 池状态；state 锁内访问，回收线程与 checkout/checkin 共用。
struct PoolState {
    /// 池内全部连接槽；空闲槽持有 transport，busy 槽为 None
    connections: HashMap<u64, ConnectionSlot>,
    /// FIFO 等待队列：按任务入队序号（queue_seq）排序，小者先得名额
    waiters: Vec<Waiter>,
    /// Session 已关闭：拒绝新 checkout，唤醒全部等待者
    closed: bool,
    /// 后台回收线程是否在运行
    reaper_running: bool,
}

/// 单条连接槽：空闲时持有 capability 与最近归还时刻，被 checkout 期间为 None。
struct ConnectionSlot {
    transport: Option<Arc<Mutex<SftpTransport>>>,
    /// 最近一次归还到空闲的时刻（仅空闲时有效）
    last_used: Instant,
}

/// 等待 checkout 的任务；携带 Session 内入队序号实现 FIFO。
struct Waiter {
    queue_seq: u64,
    wake: Arc<WaiterWake>,
}

/// 单个等待者的唤醒标志与条件变量。
struct WaiterWake {
    signaled: Mutex<bool>,
    /// 被 cancel_waiter 从队列移除并唤醒后置位：checkout 据此返回 Cancelled
    cancelled: AtomicBool,
    cond: Condvar,
}

/// checkout 失败原因：区分“Session 关闭 / 等待中被取消”（任务自行终态）
/// 与具体连接错误，供上层按不同路径迁移任务状态。
#[derive(Debug)]
pub(crate) enum CheckoutError {
    /// Session 已关闭：池不再交付任何连接，任务应静默终止
    Closed,
    /// 排队等待期间被取消：任务直接迁移到 Cancelled
    Cancelled,
    /// 建连失败等具体应用错误：任务保留结构化错误
    Connect(AppError),
}

/// 从池中 checkout 出的传输连接；checkin 时归还或淘汰。
pub(crate) struct TransferCheckout {
    pub(crate) seq: u64,
    pub(crate) transport: Arc<Mutex<SftpTransport>>,
}

impl TransferPool {
    /// 用 Arc::new_cyclic 构造连接池，使后台回收线程能持有自身引用。
    pub(crate) fn new_cyclic(
        host: HostConfig,
        verifier: HostKeyVerifier,
        connector: SftpConnector,
        clock: Arc<TransferClock>,
        idle_timeout: Duration,
    ) -> Arc<Self> {
        Arc::new_cyclic(|weak| Self {
            host,
            verifier,
            connector,
            clock,
            idle_timeout,
            state: Mutex::new(PoolState {
                connections: HashMap::new(),
                waiters: Vec::new(),
                closed: false,
                reaper_running: false,
            }),
            next_seq: AtomicU64::new(0),
            self_ref: weak.clone(),
        })
    }

    /// 获取一条传输连接：优先复用空闲连接（基础连接先取），名额未满时按需建连，
    /// 名额已满时按 Session 内 FIFO 排队等待释放。阻塞语义，只在 worker 阻塞线程调用。
    /// 排队期间被 cancel_waiter 取消时返回 CheckoutError::Cancelled，不再占用名额。
    pub(crate) fn checkout(&self, queue_seq: u64) -> Result<TransferCheckout, CheckoutError> {
        let wake = Arc::new(WaiterWake {
            signaled: Mutex::new(false),
            cancelled: AtomicBool::new(false),
            cond: Condvar::new(),
        });
        // 首次进入视为新到达：必须排在现有等待者之后；被唤醒重入时直接取名额
        let mut first_attempt = true;
        loop {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if state.closed {
                return Err(CheckoutError::Closed);
            }
            // 每次进入先回收已空闲超时的额外连接
            let now = self.clock.now();
            self.sweep_expired_locked(&mut state, now);

            // 已有等待者时新到 checkout 一律排队：不得抢占释放名额，保证 FIFO。
            // 被唤醒的等待者已取得优先级，跳过此规则直接取名额。
            if first_attempt && !state.waiters.is_empty() {
                state.waiters.push(Waiter {
                    queue_seq,
                    wake: wake.clone(),
                });
                drop(state);
                wait_for_wakeup(&wake);
                if wake.cancelled.load(Ordering::Relaxed) {
                    // 排队期间被取消：已从队列移除，立即返回不再取名额
                    return Err(CheckoutError::Cancelled);
                }
                first_attempt = false;
                continue;
            }

            // 优先复用空闲连接：基础连接（最小序号）先被取出
            if let Some(seq) = idle_min_seq(&state) {
                let slot = state.connections.get_mut(&seq).ok_or_else(|| {
                    CheckoutError::Connect(AppError::SftpChannelError("传输连接槽丢失".to_string()))
                })?;
                let transport = slot.transport.take().ok_or_else(|| {
                    CheckoutError::Connect(AppError::SftpChannelError("传输连接槽为空".to_string()))
                })?;
                return Ok(TransferCheckout { seq, transport });
            }

            // 名额未满：预留槽位后在调用线程同步建连
            if state.connections.len() < MAX_TRANSFER_CONNECTIONS_PER_SESSION {
                let seq = self.next_seq.fetch_add(1, Ordering::Relaxed);
                state.connections.insert(
                    seq,
                    ConnectionSlot {
                        transport: None,
                        last_used: now,
                    },
                );
                drop(state);
                return self.create_connection(seq);
            }

            // 名额已满：FIFO 排队等待释放
            state.waiters.push(Waiter {
                queue_seq,
                wake: wake.clone(),
            });
            drop(state);
            wait_for_wakeup(&wake);
            if wake.cancelled.load(Ordering::Relaxed) {
                // 排队期间被取消：已从队列移除，立即返回不再取名额
                return Err(CheckoutError::Cancelled);
            }
            first_attempt = false;
        }
    }

    /// 取消指定入队序号的等待者：从 FIFO 队列移除并唤醒，令其 checkout 返回
    /// CheckoutError::Cancelled。任务已取得连接（不在队列中）时为 no-op，
    /// 该任务经取消令牌在后续阶段自行终止。
    pub(crate) fn cancel_waiter(&self, queue_seq: u64) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(index) = state.waiters.iter().position(|w| w.queue_seq == queue_seq) else {
            return;
        };
        let waiter = state.waiters.remove(index);
        waiter.wake.cancelled.store(true, Ordering::Relaxed);
        signal_waiter(&waiter.wake);
    }

    /// 建连并交付 checkout；建连失败只影响本次 checkout，唤醒队首等待者重试。
    fn create_connection(&self, seq: u64) -> Result<TransferCheckout, CheckoutError> {
        let result = (self.connector)(&self.host, SftpRole::Transfer, &self.verifier);
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.closed {
            // 迟到建连结果：Session 已关闭，立即释放
            state.connections.remove(&seq);
            drop(state);
            return Err(CheckoutError::Closed);
        }
        match result {
            Ok(transport) => Ok(TransferCheckout {
                seq,
                transport: Arc::new(Mutex::new(transport)),
            }),
            Err(error) => {
                // 建连失败只影响本任务：释放预留名额并唤醒下一位等待者重试
                state.connections.remove(&seq);
                self.wake_next_waiter(&mut state);
                Err(CheckoutError::Connect(error))
            }
        }
    }

    /// 归还传输连接：失效连接直接淘汰，健康连接入池并唤醒队首等待者。
    pub(crate) fn checkin(&self, checkout: TransferCheckout, healthy: bool) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.closed || !healthy {
            // 移除失效槽位；capability 随 checkout drop 释放。
            // Session 已关闭时 connections 已整体清空，remove 为 no-op。
            state.connections.remove(&checkout.seq);
            self.wake_next_waiter(&mut state);
            return;
        }
        let now = self.clock.now();
        state.connections.insert(
            checkout.seq,
            ConnectionSlot {
                transport: Some(checkout.transport),
                last_used: now,
            },
        );
        self.wake_next_waiter(&mut state);
        // 有空闲额外连接时确保后台回收线程运行（手工时钟由测试驱动，不启动线程）
        self.ensure_reaper_locked(&mut state);
    }

    /// 关闭池：立即释放全部空闲连接，唤醒全部等待者；
    /// busy 连接由持卡 worker 归还时释放，建连中的迟到结果在 create_connection 释放。
    pub(crate) fn close(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.closed = true;
        state.connections.clear();
        for waiter in state.waiters.drain(..) {
            signal_waiter(&waiter.wake);
        }
    }

    /// 回收已空闲超时的额外连接：基础连接（最小序号）永不回收。
    fn sweep_expired_locked(&self, state: &mut PoolState, now: Instant) {
        let Some(base_seq) = state.connections.keys().min().copied() else {
            return;
        };
        let expired: Vec<u64> = state
            .connections
            .iter()
            .filter(|(seq, slot)| {
                **seq != base_seq
                    && slot.transport.is_some()
                    && is_idle_expired(slot.last_used, now, self.idle_timeout)
            })
            .map(|(seq, _)| *seq)
            .collect();
        for seq in expired {
            // transport 随槽移除 drop，SSH 连接随之关闭
            state.connections.remove(&seq);
        }
    }

    /// 系统时钟下且存在空闲额外连接时确保后台回收线程运行。
    fn ensure_reaper_locked(&self, state: &mut PoolState) {
        if !self.clock.is_system() || state.reaper_running || !has_idle_extra(state) {
            return;
        }
        state.reaper_running = true;
        let Some(pool) = self.self_ref.upgrade() else {
            return;
        };
        std::thread::spawn(move || pool.reaper_loop());
    }

    /// 后台回收循环：睡到最早到期时刻，回收后无空闲额外连接即退出；
    /// 单次睡眠上限 1 秒，保证 Session 关闭后线程尽快退出。
    fn reaper_loop(self: Arc<Self>) {
        const MAX_SLEEP: Duration = Duration::from_secs(1);
        loop {
            let sleep_for = {
                let mut state = self
                    .state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                if state.closed {
                    state.reaper_running = false;
                    return;
                }
                let Some(earliest) = earliest_idle_expiry(&state, self.idle_timeout) else {
                    state.reaper_running = false;
                    return;
                };
                earliest
                    .saturating_duration_since(self.clock.now())
                    .min(MAX_SLEEP)
            };
            std::thread::sleep(sleep_for);
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if state.closed {
                state.reaper_running = false;
                return;
            }
            let now = self.clock.now();
            self.sweep_expired_locked(&mut state, now);
            if !has_idle_extra(&state) {
                state.reaper_running = false;
                return;
            }
        }
    }

    /// 唤醒 FIFO 队首等待者（入队序号最小者）。
    fn wake_next_waiter(&self, state: &mut PoolState) {
        let Some(index) = state
            .waiters
            .iter()
            .enumerate()
            .min_by_key(|(_, waiter)| waiter.queue_seq)
            .map(|(index, _)| index)
        else {
            return;
        };
        let waiter = state.waiters.remove(index);
        signal_waiter(&waiter.wake);
    }
}

/// 置位等待者唤醒标志并通知其条件变量。
fn signal_waiter(wake: &WaiterWake) {
    *wake
        .signaled
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
    wake.cond.notify_one();
}

/// 阻塞等待被唤醒；唤醒后由调用方重新评估池状态。
fn wait_for_wakeup(wake: &WaiterWake) {
    let mut signaled = wake
        .signaled
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    while !*signaled {
        signaled = wake
            .cond
            .wait(signaled)
            .unwrap_or_else(|poisoned| poisoned.into_inner());
    }
    *signaled = false;
}

/// 返回池内空闲连接的最小序号（基础连接优先复用）。
fn idle_min_seq(state: &PoolState) -> Option<u64> {
    state
        .connections
        .iter()
        .filter(|(_, slot)| slot.transport.is_some())
        .map(|(seq, _)| *seq)
        .min()
}

/// 是否存在空闲的额外连接（除最小序号基础连接外的空闲连接）。
fn has_idle_extra(state: &PoolState) -> bool {
    let Some(base_seq) = state.connections.keys().min().copied() else {
        return false;
    };
    state
        .connections
        .iter()
        .any(|(seq, slot)| *seq != base_seq && slot.transport.is_some())
}

/// 计算最早的空闲额外连接到期时刻。
fn earliest_idle_expiry(state: &PoolState, idle_timeout: Duration) -> Option<Instant> {
    let base_seq = state.connections.keys().min().copied()?;
    state
        .connections
        .iter()
        .filter(|(seq, slot)| **seq != base_seq && slot.transport.is_some())
        .map(|(_, slot)| slot.last_used + idle_timeout)
        .min()
}

/// 判断连接自上次归还以来是否已连续空闲超过回收阈值（纯策略，供确定性测试）。
pub(crate) fn is_idle_expired(last_used: Instant, now: Instant, idle_timeout: Duration) -> bool {
    now.duration_since(last_used) >= idle_timeout
}
