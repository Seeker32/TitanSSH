use crate::core::monitor_service::MonitorService;
use crate::core::sftp_service::SftpService;
use crate::core::terminal_service;
use crate::core::terminal_service::TerminalCommand;
use crate::errors::app_error::AppError;
use crate::models::host::HostConfig;
use crate::models::monitor::{MonitorSnapshot, TaskInfo};
use crate::models::session::{SessionInfo, SessionStatus};
use crate::models::sftp::{RemoteEntry, TransferTask};
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
    sessions: HashMap<String, SessionHandle>,
    /// 独立监控服务，负责管理所有监控任务的生命周期（单一实现）
    monitor_service: MonitorService,
    /// SFTP 服务，Arc<Mutex> 包装以支持跨线程注册 session
    sftp_service: Arc<Mutex<SftpService>>,
}

impl SessionManager {
    /// 创建新的会话管理器实例
    pub fn new() -> Self {
        Self {
            sessions: HashMap::new(),
            monitor_service: MonitorService::new(),
            sftp_service: Arc::new(Mutex::new(SftpService::new())),
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

        // 克隆 host 存入 SessionHandle，terminal_service 消费原始 host
        let host_for_handle = host.clone();

        // 注册会话句柄到 HashMap
        self.sessions.insert(
            session_id.clone(),
            SessionHandle {
                meta: session_info.clone(),
                command_tx,
                shutdown: shutdown.clone(),
                host: host_for_handle,
            },
        );

        // 启动 terminal_service 工作线程（SSH 连接、PTY、终端 IO）
        // 创建 SSH session 回传通道，连接成功后将 Arc<Mutex<Session>> 注册到 sftp_service
        let (ssh_tx, ssh_rx) = std::sync::mpsc::sync_channel::<Arc<Mutex<ssh2::Session>>>(1);
        terminal_service::start_terminal_session(app, host, session_id.clone(), command_rx, shutdown, Some(ssh_tx));

        // 在后台线程中等待 SSH session 回传，成功后注册到 sftp_service
        // 使用独立线程避免阻塞 open_session 调用方
        let sftp_service = self.sftp_service.clone();
        let sid = session_id.clone();
        std::thread::spawn(move || {
            // 最多等待 30s（SSH 连接超时时间内）
            if let Ok(ssh_session) = ssh_rx.recv_timeout(std::time::Duration::from_secs(30)) {
                // SSH 连接成功后将 session 注册到 sftp_service，供后续 SFTP 操作使用
                if let Ok(mut svc) = sftp_service.lock() {
                    svc.register_session(sid, ssh_session);
                }
            }
        });

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
    /// 同时清理 sftp_service 中该会话的所有 Pending/Running 任务，推送取消状态事件。
    ///
    /// # 参数
    /// - `session_id`: 要关闭的会话 ID
    /// - `app`: Tauri 应用句柄，用于派发 sftp 任务取消事件
    pub fn close_session<R: Runtime>(&mut self, session_id: &str, app: &AppHandle<R>) -> Result<(), AppError> {
        let handle = self
            .sessions
            .remove(session_id)
            .ok_or_else(|| AppError::SessionNotFound(session_id.to_string()))?;
        // 通知所有工作线程退出
        handle.shutdown.store(true, Ordering::Relaxed);
        // 发送关闭命令到终端工作线程
        let _ = handle.command_tx.send(TerminalCommand::Close);
        // 清理 SFTP 状态，取消所有 Pending/Running 任务并推送 sftp:task_status = Cancelled
        if let Ok(mut svc) = self.sftp_service.lock() {
            svc.cleanup_session(session_id, app);
        }
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

    /// 为指定会话启动监控任务
    ///
    /// 从 SessionHandle 取出主机配置，从 secure_store 读取运行时凭据，
    /// 委托给 monitor_service 创建后台采集任务。
    /// 凭据读取失败时直接返回错误，不启动监控任务。
    ///
    /// # 参数
    /// - `session_id`: 关联的会话 ID
    /// - `app`: Tauri 应用句柄，用于派发事件
    ///
    /// # 返回
    /// 成功返回 TaskInfo，失败返回 AppError
    pub fn start_monitoring<R: Runtime>(
        &self,
        session_id: String,
        app: AppHandle<R>,
    ) -> Result<TaskInfo, AppError> {
        use crate::models::host::AuthType;
        use crate::storage::secure_store;

        // 取出主机配置
        let handle = self
            .sessions
            .get(&session_id)
            .ok_or_else(|| AppError::SessionNotFound(session_id.clone()))?;
        let host = handle.host.clone();

        // 根据认证类型从 secure_store 读取运行时凭据
        // passphrase_ref 为 None 时直接传 None，不调用 get_credential
        let (password, passphrase) = match host.auth_type {
            AuthType::Password => {
                let pw_ref = host.password_ref.as_deref()
                    .ok_or_else(|| AppError::InvalidHostConfig("密码引用为空".to_string()))?;
                let pw = secure_store::get_credential(pw_ref)?;
                (Some(pw), None)
            }
            AuthType::PrivateKey => {
                let pp = if let Some(ref pp_ref) = host.passphrase_ref {
                    Some(secure_store::get_credential(pp_ref)?)
                } else {
                    None
                };
                (None, pp)
            }
        };

        let task_info = self.monitor_service.start_monitoring(
            session_id,
            host,
            password,
            passphrase,
            app,
        );
        Ok(task_info)
    }

    /// 停止指定监控任务，委托给 monitor_service
    pub fn stop_monitoring(&self, task_id: &str) {
        self.monitor_service.stop_monitoring(task_id)
    }

    /// 获取指定会话的最新监控快照，委托给 monitor_service
    pub fn get_monitor_snapshot(&self, session_id: &str) -> Option<MonitorSnapshot> {
        self.monitor_service.get_monitor_status(session_id)
    }

    /// 列举远程目录，委托给 sftp_service
    ///
    /// # 参数
    /// - `session_id`: 关联的 SSH 会话 ID
    /// - `path`: 远程目录绝对路径
    pub fn sftp_list_dir(&self, session_id: &str, path: &str) -> Result<Vec<RemoteEntry>, AppError> {
        self.sftp_service
            .lock()
            .map_err(|e| AppError::SftpChannelError(e.to_string()))?
            .list_dir(session_id, path)
    }

    /// 发起下载任务，委托给 sftp_service
    ///
    /// # 参数
    /// - `session_id`: 关联会话 ID
    /// - `remote_path`: 远程文件完整路径
    /// - `local_path`: 本地保存路径
    /// - `app`: Tauri 应用句柄
    pub fn sftp_download<R: Runtime>(
        &mut self,
        session_id: String,
        remote_path: String,
        local_path: String,
        app: AppHandle<R>,
    ) -> Result<TransferTask, AppError> {
        self.sftp_service
            .lock()
            .map_err(|e| AppError::SftpChannelError(e.to_string()))?
            .enqueue_download(session_id, remote_path, local_path, app)
    }

    /// 发起上传任务，委托给 sftp_service
    ///
    /// # 参数
    /// - `session_id`: 关联会话 ID
    /// - `local_path`: 本地文件完整路径
    /// - `remote_path`: 远程目标目录路径
    /// - `app`: Tauri 应用句柄
    pub fn sftp_upload<R: Runtime>(
        &mut self,
        session_id: String,
        local_path: String,
        remote_path: String,
        app: AppHandle<R>,
    ) -> Result<TransferTask, AppError> {
        self.sftp_service
            .lock()
            .map_err(|e| AppError::SftpChannelError(e.to_string()))?
            .enqueue_upload(session_id, local_path, remote_path, app)
    }

    /// 取消传输任务，委托给 sftp_service
    ///
    /// # 参数
    /// - `task_id`: 要取消的任务 ID
    pub fn sftp_cancel_task(&mut self, task_id: &str) {
        if let Ok(mut svc) = self.sftp_service.lock() {
            svc.cancel_task(task_id);
        }
    }

}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::host::{AuthType, HostConfig};

    /// 构造测试用 HostConfig
    #[allow(dead_code)]
    fn make_host(id: &str) -> HostConfig {
        HostConfig {
            id: id.to_string(), name: "test".to_string(),
            host: "127.0.0.1".to_string(), port: 22,
            username: "root".to_string(),
            auth_type: AuthType::Password,
            password_ref: Some("ref".to_string()),
            private_key_path: None, passphrase_ref: None, remark: None,
        }
    }

    /// start_monitoring 对不存在的 session_id 返回 SessionNotFound 错误
    #[test]
    fn start_monitoring_unknown_session_returns_error() {
        use tauri::test::mock_app;
        let app = mock_app();
        let manager = SessionManager::new();
        let result = manager.start_monitoring("nonexistent".to_string(), app.handle().clone());
        assert!(result.is_err(), "不存在的 session_id 应返回错误");
        match result.unwrap_err() {
            AppError::SessionNotFound(id) => assert_eq!(id, "nonexistent"),
            other => panic!("期望 SessionNotFound，实际: {:?}", other),
        }
    }
}
