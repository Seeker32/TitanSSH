use crate::core::host_identity::{HostIdentityService, HostKeyVerifier};
use crate::core::monitor_service::MonitorService;
use crate::core::sftp_service::SftpService;
use crate::core::terminal_service;
use crate::core::terminal_service::TerminalCommand;
use crate::errors::app_error::AppError;
use crate::models::host::HostConfig;
use crate::models::session::{SessionInfo, SessionStatus};
use log::warn;
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
    sessions: Arc<Mutex<HashMap<String, SessionHandle>>>,
    /// 独立监控服务，负责管理所有监控任务的生命周期（单一实现）
    monitor_service: MonitorService,
    /// File Transfer module，共享 clone 只复制内部 registry 引用。
    sftp_service: SftpService,
    /// 主机身份确认服务：临时信任与 pending challenge 的单一后端权威
    identity_service: HostIdentityService,
}

impl SessionManager {
    /// 使用共享 Monitoring、File Transfer 与主机身份确认状态创建会话管理器实例
    pub fn new(
        monitor_service: MonitorService,
        sftp_service: SftpService,
        identity_service: HostIdentityService,
    ) -> Self {
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            monitor_service,
            sftp_service,
            identity_service,
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

        // 为该 Runtime Session 构建统一主机身份校验器：Terminal、SFTP、Monitoring 共用
        let verifier = self
            .identity_service
            .verifier(app.clone(), session_id.clone());

        // 与 Terminal 并行启动独立 SFTP 连接；registry 在返回前已可等待连接结果。
        self.sftp_service.register_session_with_verifier(
            session_id.clone(),
            host.clone(),
            verifier.clone(),
        );

        // 启动 terminal_service 工作线程（独立 SSH 连接、PTY、终端 IO）
        let sessions_for_cleanup = Arc::clone(&self.sessions);
        let monitor_service_for_cleanup = self.monitor_service.clone();
        let sftp_service_for_cleanup = self.sftp_service.clone();
        let identity_service_for_cleanup = self.identity_service.clone();
        let app_for_cleanup = app.clone();
        let session_id_for_cleanup = session_id.clone();
        terminal_service::start_terminal_session(
            app,
            host,
            session_id.clone(),
            command_rx,
            shutdown.clone(),
            runtime_status,
            verifier,
            Box::new(move || {
                cleanup_registered_session(
                    &sessions_for_cleanup,
                    &monitor_service_for_cleanup,
                    &sftp_service_for_cleanup,
                    &identity_service_for_cleanup,
                    &session_id_for_cleanup,
                    &app_for_cleanup,
                );
            }),
        );

        Ok(session_info)
    }

    /// 向指定会话的终端写入原始字节
    ///
    /// 将写入命令路由到对应会话的 terminal_service 工作线程。
    pub fn write_terminal(&self, session_id: &str, data: Vec<u8>) -> Result<(), AppError> {
        let command_tx = self
            .sessions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(session_id)
            .ok_or_else(|| AppError::SessionNotFound(session_id.to_string().into()))?
            .command_tx
            .clone();
        command_tx
            .send(TerminalCommand::Write(data))
            .map_err(|_| AppError::SessionNotFound(session_id.to_string().into()))
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
            .ok_or_else(|| AppError::SessionNotFound(session_id.to_string().into()))?
            .command_tx
            .clone();
        command_tx
            .send(TerminalCommand::Resize { cols, rows })
            .map_err(|_| AppError::SessionNotFound(session_id.to_string().into()))
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
        if cleanup_registered_session(
            &self.sessions,
            &self.monitor_service,
            &self.sftp_service,
            &self.identity_service,
            session_id,
            app,
        ) {
            Ok(())
        } else {
            Err(AppError::SessionNotFound(session_id.to_string().into()))
        }
    }

    /// 协调应用退出：关闭全部 Terminal、取消 File Transfer、停止 Monitoring 并撤销主机身份等待者。
    ///
    /// ExitRequested 与 Exit 可重复调用；各 service teardown 均按 registry miss 幂等处理。
    pub fn shutdown_all<R: Runtime>(&self, app: &AppHandle<R>) {
        let session_ids: Vec<String> = self
            .sessions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .keys()
            .cloned()
            .collect();
        for session_id in session_ids {
            cleanup_registered_session(
                &self.sessions,
                &self.monitor_service,
                &self.sftp_service,
                &self.identity_service,
                &session_id,
                app,
            );
        }
        self.monitor_service.stop_all(app);
        self.sftp_service.cleanup_all(app);
        self.identity_service.cancel_all(app);
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

    /// 为指定 Session 构建主机身份统一校验器，供 Monitoring 等按需启动的 capability 使用。
    pub fn host_key_verifier<R: Runtime>(
        &self,
        app: &AppHandle<R>,
        session_id: &str,
    ) -> Result<HostKeyVerifier, AppError> {
        // 校验会话存在，避免为已关闭会话发放校验器
        self.host_config(session_id)?;
        Ok(self
            .identity_service
            .verifier(app.clone(), session_id.to_string()))
    }

    /// 主机身份确认服务句柄，供命令层接受/拒绝决定使用。
    pub fn identity_service(&self) -> &HostIdentityService {
        &self.identity_service
    }

    /// 返回指定 Session 的主机配置副本，供所属 module 在锁外启动工作。
    pub fn host_config(&self, session_id: &str) -> Result<HostConfig, AppError> {
        self.sessions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(session_id)
            .map(|handle| handle.host.clone())
            .ok_or_else(|| AppError::SessionNotFound(session_id.to_string().into()))
    }

    /// 测试构造：直接注册一个会话句柄（绕开真实 SSH 连接），供命令层测试
    /// 会话存在性判定。
    #[cfg(test)]
    pub(crate) fn insert_session_for_test(&self, session_id: &str, host: HostConfig) {
        let _ = self.insert_session_for_test_with_receiver(session_id, host);
    }

    /// 测试构造：注册会话并保留终端命令接收端，验证输入字节未被改写。
    #[cfg(test)]
    pub(crate) fn insert_session_for_test_with_receiver(
        &self,
        session_id: &str,
        host: HostConfig,
    ) -> mpsc::Receiver<TerminalCommand> {
        let (command_tx, command_rx) = mpsc::channel();
        self.sessions.lock().unwrap().insert(
            session_id.to_string(),
            SessionHandle {
                meta: SessionInfo {
                    session_id: session_id.to_string(),
                    host_id: host.id.clone(),
                    host: host.host.clone(),
                    port: host.port,
                    username: host.username.clone(),
                    status: SessionStatus::Connecting,
                    created_at: 1_710_000_000_000,
                },
                runtime_status: Arc::new(Mutex::new(SessionStatus::Connecting)),
                command_tx,
                shutdown: Arc::new(AtomicBool::new(false)),
                host,
            },
        );
        command_rx
    }
}

/// 回收已注册会话及其所属的所有后台资源。
///
/// 显式关闭和终端工作线程退出共享这一路径；若另一方已先完成回收，返回 false，
/// 以保证并发 teardown 幂等且不会重复推送取消事件。
fn cleanup_registered_session<R: Runtime>(
    sessions: &Arc<Mutex<HashMap<String, SessionHandle>>>,
    monitor_service: &MonitorService,
    sftp_service: &SftpService,
    identity_service: &HostIdentityService,
    session_id: &str,
    app: &AppHandle<R>,
) -> bool {
    let Some(handle) = sessions
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .remove(session_id)
    else {
        return false;
    };

    // 取消该 Session 的主机身份等待者并清除临时信任，等待中的连接不得进入认证
    identity_service.cancel_session(app, session_id);
    // 通知所有工作线程退出
    handle.shutdown.store(true, Ordering::Relaxed);
    // 发送关闭命令到终端工作线程；接收端已退出时仍继续其余回收，但记录可观测诊断。
    if let Err(error) = handle.command_tx.send(TerminalCommand::Close) {
        warn!(
            "[session:{}][diagnostic] Failed to deliver terminal close command: {}",
            session_id, error
        );
    }
    // 停止该会话的全部监控任务（每个任务补发 Done 终态事件）
    monitor_service.stop_session(app, session_id);
    // 清理 SFTP 状态，取消所有 Pending/Running 任务并推送 sftp:task_status = Cancelled
    sftp_service.cleanup_session(session_id, app);
    true
}

#[cfg(test)]
#[path = "session_manager_test.rs"]
mod tests;
