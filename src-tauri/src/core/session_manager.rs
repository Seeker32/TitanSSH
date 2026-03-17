use crate::core::monitor_service::MonitorService;
use crate::core::terminal_service;
use crate::core::terminal_service::TerminalCommand;
use crate::errors::app_error::AppError;
use crate::models::host::HostConfig;
use crate::models::monitor::{MonitorSnapshot, TaskInfo};
use crate::models::session::{SessionInfo, SessionStatus};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::Arc;
use tauri::AppHandle;
use uuid::Uuid;

/// SSH 会话句柄，包含会话元数据、命令通道和关闭标志
#[derive(Clone)]
pub struct SessionHandle {
    /// 会话基本信息（ID、主机、状态等）
    pub meta: SessionInfo,
    /// 向终端工作线程发送命令的通道发送端
    pub command_tx: Sender<TerminalCommand>,
    /// 会话关闭标志，设置为 true 时通知所有工作线程退出
    pub shutdown: Arc<AtomicBool>,
}

/// 会话管理器（纯协调层）
///
/// 只负责真实会话的注册、索引与生命周期协调，
/// 不直接承担终端 IO 或监控采集逻辑。
/// 监控能力统一由 monitor_service 提供，不存在双轨实现。
pub struct SessionManager {
    /// 存储所有活跃会话的 HashMap，键为 session_id
    sessions: HashMap<String, SessionHandle>,
    /// 独立监控服务，负责管理所有监控任务的生命周期（单一实现）
    monitor_service: MonitorService,
}

impl SessionManager {
    /// 创建新的会话管理器实例
    pub fn new() -> Self {
        Self {
            sessions: HashMap::new(),
            monitor_service: MonitorService::new(),
        }
    }

    /// 打开一个新的 SSH 会话
    ///
    /// 生成唯一 session_id，创建 SessionInfo，启动 terminal_service 工作线程，
    /// 并将会话句柄注册到内部 HashMap。
    /// 监控不在此处自动启动，由前端显式调用 start_monitoring。
    ///
    /// # 参数
    /// - `app`: Tauri 应用句柄，用于派发事件
    /// - `host`: 主机配置（不含明文凭据）
    ///
    /// # 返回
    /// 成功返回 SessionInfo，失败返回 AppError
    pub fn open_session(
        &mut self,
        app: AppHandle,
        host: HostConfig,
    ) -> Result<SessionInfo, AppError> {
        // 生成唯一会话 ID
        let session_id = Uuid::new_v4().to_string();

        // 创建会话信息，created_at 使用毫秒时间戳
        let session_info = SessionInfo {
            session_id: session_id.clone(),
            host_id: host.id.clone(),
            host: host.host.clone(),
            port: host.port,
            username: host.username.clone(),
            status: SessionStatus::Connecting,
            created_at: chrono::Utc::now().timestamp_millis(),
        };

        // 创建终端命令通道
        let (command_tx, command_rx) = mpsc::channel();
        // 创建共享关闭标志
        let shutdown = Arc::new(AtomicBool::new(false));

        // 注册会话句柄到 HashMap
        self.sessions.insert(
            session_id.clone(),
            SessionHandle {
                meta: session_info.clone(),
                command_tx,
                shutdown: shutdown.clone(),
            },
        );

        // 启动 terminal_service 工作线程（SSH 连接、PTY、终端 IO）
        terminal_service::start_terminal_session(
            app,
            host,
            session_id,
            command_rx,
            shutdown,
        );

        Ok(session_info)
    }

    /// 更新指定会话的状态元数据，保持后端 HashMap 与实际运行态一致
    ///
    /// 由 session 命令层在收到 terminal_service 状态变更后调用。
    /// 若会话不存在则静默忽略。
    pub fn update_session_status(&mut self, session_id: &str, status: SessionStatus) {
        if let Some(handle) = self.sessions.get_mut(session_id) {
            handle.meta.status = status;
        }
    }

    /// 向指定会话的终端写入数据
    ///
    /// 将写入命令路由到对应会话的 terminal_service 工作线程。
    pub fn write_terminal(&self, session_id: &str, data: String) -> Result<(), AppError> {
        let handle = self
            .sessions
            .get(session_id)
            .ok_or_else(|| AppError::SessionNotFound(session_id.to_string()))?;
        handle
            .command_tx
            .send(TerminalCommand::Write(data))
            .map_err(|error| AppError::IoError(std::io::Error::other(error.to_string())))
    }

    /// 调整指定会话的终端大小
    ///
    /// 将 Resize 命令路由到对应会话的 terminal_service 工作线程。
    pub fn resize_terminal(&self, session_id: &str, cols: u32, rows: u32) -> Result<(), AppError> {
        let handle = self
            .sessions
            .get(session_id)
            .ok_or_else(|| AppError::SessionNotFound(session_id.to_string()))?;
        handle
            .command_tx
            .send(TerminalCommand::Resize { cols, rows })
            .map_err(|error| AppError::IoError(std::io::Error::other(error.to_string())))
    }

    /// 关闭指定会话
    ///
    /// 设置 shutdown 标志，发送 Close 命令，并从 HashMap 中移除会话句柄。
    pub fn close_session(&mut self, session_id: &str) -> Result<(), AppError> {
        let handle = self
            .sessions
            .remove(session_id)
            .ok_or_else(|| AppError::SessionNotFound(session_id.to_string()))?;
        // 通知所有工作线程退出
        handle.shutdown.store(true, Ordering::Relaxed);
        // 发送关闭命令到终端工作线程
        let _ = handle.command_tx.send(TerminalCommand::Close);
        Ok(())
    }

    /// 获取所有活跃会话的列表
    ///
    /// 返回内部 HashMap 中所有会话的 SessionInfo 副本，状态已通过 update_session_status 同步。
    pub fn list_sessions(&self) -> Vec<SessionInfo> {
        self.sessions
            .values()
            .map(|handle| handle.meta.clone())
            .collect()
    }

    /// 为指定会话启动监控任务，委托给 monitor_service（单一监控实现）
    pub fn start_monitoring(&self, session_id: String, app: AppHandle) -> TaskInfo {
        self.monitor_service.start_monitoring(session_id, app)
    }

    /// 停止指定监控任务，委托给 monitor_service
    pub fn stop_monitoring(&self, task_id: &str) {
        self.monitor_service.stop_monitoring(task_id)
    }

    /// 获取指定会话的最新监控快照，委托给 monitor_service
    pub fn get_monitor_snapshot(&self, session_id: &str) -> Option<MonitorSnapshot> {
        self.monitor_service.get_monitor_status(session_id)
    }

    /// 仅供测试使用：直接向 HashMap 插入伪造的会话句柄，绕过真实 SSH 连接
    ///
    /// 允许属性测试在不依赖 AppHandle 或真实网络的情况下验证 HashMap 与 list_sessions 的一致性。
    #[cfg(test)]
    pub fn insert_session_for_test(&mut self, session_info: SessionInfo) {
        let (command_tx, _command_rx) = mpsc::channel();
        let shutdown = Arc::new(AtomicBool::new(false));
        self.sessions.insert(
            session_info.session_id.clone(),
            SessionHandle {
                meta: session_info,
                command_tx,
                shutdown,
            },
        );
    }

    /// 仅供测试使用：返回内部 HashMap 中所有会话 ID 的集合
    ///
    /// 用于属性测试中直接对比 HashMap 键集与 list_sessions 返回结果。
    #[cfg(test)]
    pub fn session_ids_in_map(&self) -> std::collections::HashSet<String> {
        self.sessions.keys().cloned().collect()
    }

    /// 仅供测试使用：生成真实 UUID 并注册会话，绕过 AppHandle 和真实 SSH 连接
    ///
    /// 复现 open_session 中的 UUID 生成与 HashMap 注册逻辑，
    /// 允许属性测试在无 Tauri 运行时的情况下验证 session_id 唯一性。
    #[cfg(test)]
    pub fn open_session_for_test(&mut self, host_id: &str) -> String {
        let session_id = Uuid::new_v4().to_string();
        let session_info = SessionInfo {
            session_id: session_id.clone(),
            host_id: host_id.to_string(),
            host: "127.0.0.1".to_string(),
            port: 22,
            username: "test".to_string(),
            status: SessionStatus::Connecting,
            created_at: 1_710_000_000_000_i64,
        };
        self.insert_session_for_test(session_info);
        session_id
    }
}

#[cfg(test)]
mod tests {
    use super::SessionManager;
    use crate::models::session::{SessionInfo, SessionStatus};
    use proptest::prelude::*;
    use std::collections::HashSet;

    /// 生成非空字母数字字符串的策略（1-32 个字符）
    fn arb_nonempty_string() -> impl Strategy<Value = String> {
        "[a-zA-Z0-9_\\-]{1,32}".prop_map(|s| s)
    }

    /// 生成合法端口号的策略
    fn arb_port() -> impl Strategy<Value = u16> {
        1u16..=65535u16
    }

    /// 生成任意合法 SessionInfo 的策略
    fn arb_session_info() -> impl Strategy<Value = SessionInfo> {
        (
            arb_nonempty_string(),
            arb_nonempty_string(),
            arb_nonempty_string(),
            arb_port(),
            arb_nonempty_string(),
        )
            .prop_map(|(id_suffix, host_id, host, port, username)| SessionInfo {
                session_id: format!("sess-{}", id_suffix),
                host_id,
                host,
                port,
                username,
                status: SessionStatus::Connecting,
                created_at: 1_710_000_000_000_i64,
            })
    }

    /// 会话操作枚举，用于生成任意 open/close 操作序列
    #[derive(Debug, Clone)]
    enum SessionOp {
        Open(SessionInfo),
        Close(String),
    }

    /// 生成任意操作序列的策略（1-20 个操作）
    fn arb_session_ops() -> impl Strategy<Value = Vec<SessionOp>> {
        prop::collection::vec(
            prop_oneof![
                arb_session_info().prop_map(SessionOp::Open),
                arb_nonempty_string().prop_map(|s| SessionOp::Close(format!("sess-{}", s))),
            ],
            1..=20,
        )
    }

    proptest! {
        /// **验证: 需求 7.2** — Session ID 唯一
        #[test]
        fn prop_session_ids_are_unique(n in 1usize..=20usize) {
            let mut manager = SessionManager::new();
            let ids: Vec<String> = (0..n)
                .map(|i| manager.open_session_for_test(&format!("host-{}", i)))
                .collect();
            let unique_ids: HashSet<String> = ids.iter().cloned().collect();
            prop_assert_eq!(
                unique_ids.len(),
                ids.len(),
                "连续打开 {} 个会话后，所有 session_id 必须互不相同",
                n
            );
        }

        /// **验证: 需求 7.6, 7.7** — 真实会话集合与 list_sessions 一致
        #[test]
        fn prop_list_sessions_consistent_with_internal_map(ops in arb_session_ops()) {
            let mut manager = SessionManager::new();
            for op in &ops {
                match op {
                    SessionOp::Open(session_info) => {
                        manager.insert_session_for_test(session_info.clone());
                    }
                    SessionOp::Close(session_id) => {
                        let _ = manager.close_session(session_id);
                    }
                }
            }
            let map_ids: HashSet<String> = manager.session_ids_in_map();
            let listed_sessions = manager.list_sessions();
            let listed_ids: HashSet<String> =
                listed_sessions.iter().map(|s| s.session_id.clone()).collect();

            prop_assert_eq!(
                &listed_ids,
                &map_ids,
                "list_sessions 返回的 session_id 集合必须与内部 HashMap 键集完全一致"
            );
            for session in &listed_sessions {
                prop_assert!(
                    !session.session_id.starts_with("home"),
                    "list_sessions 不得包含首页视图 ID: {}",
                    session.session_id
                );
                prop_assert!(
                    !session.session_id.starts_with("ui-"),
                    "list_sessions 不得包含 UI 视图 ID: {}",
                    session.session_id
                );
            }
            prop_assert_eq!(
                listed_sessions.len(),
                listed_ids.len(),
                "list_sessions 返回列表中不得有重复的 session_id"
            );
        }

        /// **验证: P1-1** — update_session_status 正确同步后端元数据
        ///
        /// 插入会话后更新其状态，验证 list_sessions 返回的状态与更新值一致。
        #[test]
        fn prop_update_session_status_reflects_in_list(
            id_suffix in "[a-zA-Z0-9]{1,16}",
        ) {
            let mut manager = SessionManager::new();
            let session_id = format!("sess-{}", id_suffix);
            let session_info = SessionInfo {
                session_id: session_id.clone(),
                host_id: "host-1".to_string(),
                host: "127.0.0.1".to_string(),
                port: 22,
                username: "test".to_string(),
                status: SessionStatus::Connecting,
                created_at: 1_710_000_000_000_i64,
            };
            manager.insert_session_for_test(session_info);

            // 更新为 Connected
            manager.update_session_status(&session_id, SessionStatus::Connected);
            let sessions = manager.list_sessions();
            let found = sessions.iter().find(|s| s.session_id == session_id);
            prop_assert!(found.is_some(), "会话应存在于 list_sessions 中");
            prop_assert_eq!(
                found.unwrap().status.clone(),
                SessionStatus::Connected,
                "update_session_status 后 list_sessions 应返回更新后的状态"
            );

            // 再次更新为 Disconnected
            manager.update_session_status(&session_id, SessionStatus::Disconnected);
            let sessions2 = manager.list_sessions();
            let found2 = sessions2.iter().find(|s| s.session_id == session_id);
            prop_assert_eq!(
                found2.unwrap().status.clone(),
                SessionStatus::Disconnected,
                "二次更新后状态应为 Disconnected"
            );
        }
    }

    #[test]
    fn update_status_for_nonexistent_session_is_silent() {
        // 对不存在的会话调用 update_session_status 应静默忽略，不 panic
        let mut manager = SessionManager::new();
        manager.update_session_status("nonexistent", SessionStatus::Connected);
        assert!(manager.list_sessions().is_empty());
    }
}
