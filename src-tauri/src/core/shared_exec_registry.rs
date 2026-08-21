use crate::core::ssh_transport::ExecTransport;
use crate::errors::app_error::AppError;
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::sync::{Arc, Mutex};

/// 共享 exec 连接条目：可反复派生共享同一底层 SSH 连接的 exec capability。
///
/// 生产实现是 ssh_transport 的 `SharedExecConnection`（底层 `Arc<Mutex<Session>>`
/// 串行化 execute）；测试注入内存条目，业务逻辑测试不感知注册表存在。
pub(crate) trait ExecConnectionEntry: Send + Sync {
    /// 派生一个共享底层连接的 exec capability。
    fn exec_transport(&self) -> ExecTransport;
}

/// 按 sessionId 键的共享 exec 连接注册表（刻意极薄：取用即插入 / 回收）。
///
/// 连接是纯传输层基础设施，被多个采样服务（monitor / process）共享；
/// 生命周期跟 Session 走：首个消费者取用时建立，session teardown 时
/// `remove` 回收。回收后仍在途的 capability 释放时底层连接才真正关闭，
/// 因此不会打断正在执行的采集，也不会泄漏连接。
#[derive(Clone)]
pub struct SharedExecRegistry {
    state: Arc<Mutex<RegistryState>>,
}

/// 注册表状态与回收世代；世代用于丢弃 teardown 期间迟到的建连结果。
struct RegistryState {
    entries: HashMap<String, Arc<dyn ExecConnectionEntry>>,
    clear_epoch: u64,
    session_epochs: HashMap<String, u64>,
}

impl SharedExecRegistry {
    /// 创建空注册表。
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(RegistryState {
                entries: HashMap::new(),
                clear_epoch: 0,
                session_epochs: HashMap::new(),
            })),
        }
    }

    /// 插入会话的共享连接；已有条目时保留先插入者并返回 false。
    #[allow(dead_code)]
    pub(crate) fn insert<E>(&self, session_id: &str, entry: E) -> bool
    where
        E: ExecConnectionEntry + 'static,
    {
        let mut state = lock_state(&self.state);
        match state.entries.entry(session_id.to_string()) {
            Entry::Occupied(_) => false,
            Entry::Vacant(slot) => {
                slot.insert(Arc::new(entry));
                true
            }
        }
    }

    /// 取用会话的共享连接；不存在时经 `connect` 建立并插入。
    ///
    /// # 参数
    /// - `session_id`: 连接归属的会话 ID
    /// - `connect`: 缺失时的建连动作（含主机身份校验），失败原样向上传播
    ///
    /// # 返回
    /// 共享该会话底层连接的 exec capability；建连失败时返回错误且不缓存。
    ///
    /// # 副作用
    /// 首次取用会把新连接插入注册表；建连在注册表锁外执行，并发取用
    /// 可能各自建连，但只有先插入者留存——后来者的连接随即释放关闭。
    pub fn resolve<E, F>(&self, session_id: &str, connect: F) -> Result<ExecTransport, AppError>
    where
        E: ExecConnectionEntry + 'static,
        F: FnOnce() -> Result<E, AppError>,
    {
        // 快路径：命中已有连接直接派生 capability，不进入建连分支
        let (clear_epoch, session_epoch) = {
            let state = lock_state(&self.state);
            if let Some(entry) = state.entries.get(session_id) {
                return Ok(entry.exec_transport());
            }
            (
                state.clear_epoch,
                state.session_epochs.get(session_id).copied().unwrap_or(0),
            )
        };

        // 建连（可能长达连接超时）不得持有注册表锁，避免阻塞其他会话的取用/回收
        let established = Arc::new(connect()?);
        let mut state = lock_state(&self.state);
        if state.clear_epoch != clear_epoch
            || state.session_epochs.get(session_id).copied().unwrap_or(0) != session_epoch
        {
            // teardown 已先发生：把 capability 交给当前调用者，离开 worker 后自然释放，
            // 但绝不重新插入注册表。
            return Ok(established.exec_transport());
        }
        Ok(match state.entries.entry(session_id.to_string()) {
            Entry::Occupied(existing) => {
                // 并发建连输家：保留先插入者，本连接随后释放关闭
                existing.get().exec_transport()
            }
            Entry::Vacant(slot) => {
                let transport = established.exec_transport();
                slot.insert(established);
                transport
            }
        })
    }

    /// 回收会话的共享连接（session teardown）；返回是否确实移除了条目。
    ///
    /// 移除的只是注册表引用；在途 capability 持有底层连接直至其释放，
    /// 因此调用本方法不会打断正在执行的采集。
    pub fn remove(&self, session_id: &str) -> bool {
        let mut state = lock_state(&self.state);
        let removed = state.entries.remove(session_id).is_some();
        let epoch = state
            .session_epochs
            .entry(session_id.to_string())
            .or_default();
        *epoch = epoch.saturating_add(1);
        removed
    }

    /// 批量回收全部条目（应用退出兜底；逐会话 teardown 之后的最后防线）。
    pub fn clear(&self) {
        let mut state = lock_state(&self.state);
        state.entries.clear();
        state.clear_epoch = state.clear_epoch.saturating_add(1);
    }

    /// 注册表是否仍持有该会话的连接（测试观测用）。
    #[cfg(test)]
    pub(crate) fn contains(&self, session_id: &str) -> bool {
        lock_state(&self.state).entries.contains_key(session_id)
    }
}

impl Default for SharedExecRegistry {
    /// 等价于 `new`。
    fn default() -> Self {
        Self::new()
    }
}

/// 毒化容忍锁：注册表是自洽的可替换状态，持锁线程 panic 后恢复内部值继续服务。
fn lock_state(state: &Arc<Mutex<RegistryState>>) -> std::sync::MutexGuard<'_, RegistryState> {
    state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
#[path = "shared_exec_registry_test.rs"]
mod tests;
