use crate::core::monitor_service::MonitorService;
use crate::core::sftp_service::SftpService;
use crate::core::terminal_service;
use crate::core::terminal_service::TerminalCommand;
use crate::errors::app_error::AppError;
use crate::models::host::HostConfig;
use crate::models::session::{SessionInfo, SessionStatus};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use tauri::{AppHandle, Runtime};
use uuid::Uuid;

/// SSH 会话句柄，包含会话元数据、命令通道、关闭标志和主机配置
#[derive(Clone)]
pub struct SessionHandle {
    /// 会话基本信息（ID、主机、状态等）
    pub meta: SessionInfo,
    /// 后端运行时状态，由终端工作线程更新，list_sessions 直接读取
    pub runtime_status: Arc<Mutex<SessionStatus>>,
    /// 向终端工作线程发送命令的通道发送端
    pub command_tx: Sender<TerminalCommand>,
    /// 会话关闭标志，设置为 true 时通知所有工作线程退出
    pub shutdown: Arc<AtomicBool>,
    /// 主机配置（不含明文凭据），供 start_monitoring 读取
    pub host: HostConfig,
}

/// 会话管理器（纯协调层）
///
/// 只负责真实会话的注册、索引与生命周期协调，
/// 不直接承担终端 IO 或监控采集逻辑。
/// 监控能力统一由 monitor_service 提供，不存在双轨实现。
pub struct SessionManager {
    /// 存储所有活跃会话的 HashMap，键为 session_id
    sessions: Mutex<HashMap<String, SessionHandle>>,
    /// 独立监控服务，负责管理所有监控任务的生命周期（单一实现）
    monitor_service: MonitorService,
    /// File Transfer module，共享 clone 只复制内部 registry 引用。
    sftp_service: SftpService,
}

impl SessionManager {
    /// 使用共享 Monitoring 与 File Transfer 状态创建会话管理器实例
    pub fn new(monitor_service: MonitorService, sftp_service: SftpService) -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
            monitor_service,
            sftp_service,
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
    pub fn open_session<R: Runtime>(
        &self,
        app: AppHandle<R>,
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
        // 创建后端权威运行时状态，终端工作线程与会话索引共享
        let runtime_status = Arc::new(Mutex::new(SessionStatus::Connecting));

        // 克隆 host 存入 SessionHandle，terminal_service 消费原始 host
        let host_for_handle = host.clone();

        // 注册会话句柄到 HashMap
        self.sessions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(
                session_id.clone(),
                SessionHandle {
                    meta: session_info.clone(),
                    runtime_status: runtime_status.clone(),
                    command_tx,
                    shutdown: shutdown.clone(),
                    host: host_for_handle,
                },
            );

        // 与 Terminal 并行启动独立 SFTP 连接；registry 在返回前已可等待连接结果。
        self.sftp_service
            .register_session(session_id.clone(), host.clone());

        // 启动 terminal_service 工作线程（独立 SSH 连接、PTY、终端 IO）
        terminal_service::start_terminal_session(
            app,
            host,
            session_id.clone(),
            command_rx,
            shutdown.clone(),
            runtime_status,
        );

        Ok(session_info)
    }

    /// 向指定会话的终端写入数据
    ///
    /// 将写入命令路由到对应会话的 terminal_service 工作线程。
    pub fn write_terminal(&self, session_id: &str, data: String) -> Result<(), AppError> {
        let command_tx = self
            .sessions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(session_id)
            .ok_or_else(|| AppError::SessionNotFound(session_id.to_string()))?
            .command_tx
            .clone();
        command_tx
            .send(TerminalCommand::Write(data))
            .map_err(|error| AppError::IoError(std::io::Error::other(error.to_string())))
    }

    /// 调整指定会话的终端大小
    ///
    /// 将 Resize 命令路由到对应会话的 terminal_service 工作线程。
    pub fn resize_terminal(&self, session_id: &str, cols: u32, rows: u32) -> Result<(), AppError> {
        let command_tx = self
            .sessions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(session_id)
            .ok_or_else(|| AppError::SessionNotFound(session_id.to_string()))?
            .command_tx
            .clone();
        command_tx
            .send(TerminalCommand::Resize { cols, rows })
            .map_err(|error| AppError::IoError(std::io::Error::other(error.to_string())))
    }

    /// 关闭指定会话
    ///
    /// 设置 shutdown 标志，发送 Close 命令，并从 HashMap 中移除会话句柄。
    /// 同时清理 sftp_service 中该会话的所有 Pending/Running 任务，推送取消状态事件。
    ///
    /// # 参数
    /// - `session_id`: 要关闭的会话 ID
    /// - `app`: Tauri 应用句柄，用于派发 sftp 任务取消事件
    pub fn close_session<R: Runtime>(
        &self,
        session_id: &str,
        app: &AppHandle<R>,
    ) -> Result<(), AppError> {
        let handle = self
            .sessions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(session_id)
            .ok_or_else(|| AppError::SessionNotFound(session_id.to_string()))?;
        // 通知所有工作线程退出
        handle.shutdown.store(true, Ordering::Relaxed);
        // 发送关闭命令到终端工作线程
        let _ = handle.command_tx.send(TerminalCommand::Close);
        // 停止该会话的全部监控任务，teardown 不再依赖前端调用顺序
        self.monitor_service.stop_session(session_id);
        // 清理 SFTP 状态，取消所有 Pending/Running 任务并推送 sftp:task_status = Cancelled
        self.sftp_service.cleanup_session(session_id, app);
        Ok(())
    }

    /// 获取所有活跃会话的列表
    ///
    /// 返回内部 HashMap 中所有会话的 SessionInfo 副本，状态直接读取后端运行时事实。
    pub fn list_sessions(&self) -> Vec<SessionInfo> {
        self.sessions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .values()
            .map(|handle| {
                let mut info = handle.meta.clone();
                info.status = handle
                    .runtime_status
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .clone();
                info
            })
            .collect()
    }

    /// 返回指定 Session 的主机配置副本，供所属 module 在锁外启动工作。
    pub fn host_config(&self, session_id: &str) -> Result<HostConfig, AppError> {
        self.sessions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(session_id)
            .map(|handle| handle.host.clone())
            .ok_or_else(|| AppError::SessionNotFound(session_id.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::host::{AuthType, HostConfig};

    /// 创建共享运行时状态一致的 SessionManager。
    fn make_manager() -> SessionManager {
        SessionManager::new(MonitorService::new(), SftpService::new())
    }

    /// 构造测试用 HostConfig
    #[allow(dead_code)]
    fn make_host(id: &str) -> HostConfig {
        HostConfig {
            id: id.to_string(),
            name: "test".to_string(),
            host: "127.0.0.1".to_string(),
            port: 22,
            username: "root".to_string(),
            auth_type: AuthType::Password,
            password_ref: Some("ref".to_string()),
            private_key_path: None,
            passphrase_ref: None,
            remark: None,
            group: String::new(),
        }
    }

    /// host_config 对不存在的 session_id 返回 SessionNotFound 错误
    #[test]
    fn host_config_unknown_session_returns_error() {
        let manager = make_manager();
        let result = manager.host_config("nonexistent");
        assert!(result.is_err(), "不存在的 session_id 应返回错误");
        match result.unwrap_err() {
            AppError::SessionNotFound(id) => assert_eq!(id, "nonexistent"),
            other => panic!("期望 SessionNotFound，实际: {:?}", other),
        }
    }

    /// list_sessions 必须读取后端运行时状态，不依赖前端回写。
    #[test]
    fn list_sessions_reads_backend_runtime_status() {
        let manager = make_manager();
        let (command_tx, _command_rx) = mpsc::channel();
        let session_id = "session-runtime-status".to_string();

        manager.sessions.lock().unwrap().insert(
            session_id.clone(),
            SessionHandle {
                meta: SessionInfo {
                    session_id: session_id.clone(),
                    host_id: "host-1".to_string(),
                    host: "127.0.0.1".to_string(),
                    port: 22,
                    username: "root".to_string(),
                    status: SessionStatus::Connecting,
                    created_at: 1_710_000_000_000,
                },
                runtime_status: Arc::new(Mutex::new(SessionStatus::Connected)),
                command_tx,
                shutdown: Arc::new(AtomicBool::new(false)),
                host: make_host("host-1"),
            },
        );

        let sessions = manager.list_sessions();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].status, SessionStatus::Connected);
    }

    /// Session 打开返回前必须注册 SFTP 状态槽，真实连接在后台并行进行。
    #[test]
    fn open_session_registers_sftp_connection_slot() {
        use tauri::test::mock_app;

        let app = mock_app();
        let sftp_service = SftpService::with_connector(|_| {
            Err(AppError::SshConnectionError(
                "expected test failure".to_string(),
            ))
        });
        let manager = SessionManager::new(MonitorService::new(), sftp_service.clone());

        let session = manager
            .open_session(app.handle().clone(), make_host("host-1"))
            .unwrap();

        assert!(sftp_service.has_session(&session.session_id));
    }

    /// close_session 必须在后端一次性停止 Terminal、Monitoring 并清理 File Transfer。
    #[test]
    fn close_session_tears_down_all_backend_work() {
        use crate::core::monitor_service::MonitorTaskHandle;
        use crate::models::monitor::{TaskInfo, TaskStatus};
        use tauri::test::mock_app;

        let app = mock_app();
        let monitor_service = MonitorService::new();
        let sftp_service = SftpService::with_connector(|_| {
            Err(AppError::SshConnectionError(
                "expected test failure".to_string(),
            ))
        });
        let manager = SessionManager::new(monitor_service.clone(), sftp_service.clone());
        let session_id = "session-close-all".to_string();
        let terminal_shutdown = Arc::new(AtomicBool::new(false));
        let monitor_shutdown = Arc::new(AtomicBool::new(false));
        let (command_tx, _command_rx) = mpsc::channel();

        manager.sessions.lock().unwrap().insert(
            session_id.clone(),
            SessionHandle {
                meta: SessionInfo {
                    session_id: session_id.clone(),
                    host_id: "host-1".to_string(),
                    host: "127.0.0.1".to_string(),
                    port: 22,
                    username: "root".to_string(),
                    status: SessionStatus::Connected,
                    created_at: 1_710_000_000_000,
                },
                runtime_status: Arc::new(Mutex::new(SessionStatus::Connected)),
                command_tx,
                shutdown: terminal_shutdown.clone(),
                host: make_host("host-1"),
            },
        );
        monitor_service.tasks.lock().unwrap().insert(
            "monitor-task".to_string(),
            MonitorTaskHandle {
                task_info: TaskInfo {
                    task_id: "monitor-task".to_string(),
                    task_type: "monitor".to_string(),
                    session_id: Some(session_id.clone()),
                    status: TaskStatus::Running,
                    created_at: 1_710_000_000_000,
                },
                shutdown: monitor_shutdown.clone(),
            },
        );
        sftp_service.register_session(session_id.clone(), make_host("host-1"));

        manager
            .close_session(&session_id, &app.handle().clone())
            .unwrap();

        assert!(terminal_shutdown.load(Ordering::Relaxed));
        assert!(monitor_shutdown.load(Ordering::Acquire));
        assert!(monitor_service.tasks.lock().unwrap().is_empty());
        assert!(!sftp_service.has_session(&session_id));
    }
}
