// 导入核心模块
use crate::core::{monitor_worker, ssh_client, terminal_bridge};
use crate::errors::app_error::AppError;
use crate::models::host::HostConfig;
use crate::models::monitor::ServerStatus;
use crate::models::session::{SessionInfo, SessionStatus};
use serde::Serialize;
use std::collections::HashMap;
use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use tauri::{AppHandle, Emitter};
use uuid::Uuid;

/// SSH会话的句柄，包含会话元数据、命令通道和关闭标志
#[derive(Clone)]
pub struct SessionHandle {
    /// 会话基本信息（ID、主机、状态等）
    pub meta: SessionInfo,
    /// 用于向工作线程发送命令的通道发送端
    pub command_tx: Sender<SessionCommand>,
    /// 会话关闭标志，用于通知工作线程停止
    pub shutdown: Arc<AtomicBool>,
    /// 最新的服务器状态监控数据
    pub latest_status: Option<ServerStatus>,
}

/// 会话命令枚举，用于前端与终端工作线程通信
#[derive(Clone)]
pub enum SessionCommand {
    /// 向终端写入数据
    Write(String),
    /// 调整终端大小（列数、行数）
    Resize { cols: u32, rows: u32 },
    /// 关闭会话
    Close,
}

/// 会话状态变更事件，用于向前端通知会话状态变化
#[derive(Debug, Clone, Serialize)]
pub struct SessionStatusEvent {
    /// 会话唯一标识符
    pub session_id: String,
    /// 当前会话状态
    pub status: SessionStatus,
    /// 状态变更的附加消息（如错误信息）
    pub message: Option<String>,
}

/// 终端数据事件，用于向前端推送终端输出
#[derive(Debug, Clone, Serialize)]
pub struct TerminalDataEvent {
    /// 会话唯一标识符
    pub session_id: String,
    /// 终端输出的数据内容
    pub data: String,
}

/// 会话管理器，管理所有活跃的SSH会话
pub struct SessionManager {
    /// 存储所有会话的HashMap，键为会话ID
    sessions: HashMap<String, SessionHandle>,
}

impl SessionManager {
    /// 创建一个新的会话管理器实例
    pub fn new() -> Self {
        Self {
            sessions: HashMap::new(),
        }
    }

    /// 打开一个新的SSH会话
    ///
    /// # 参数
    /// - `app`: Tauri应用句柄，用于发送事件到前端
    /// - `host`: 主机配置信息
    ///
    /// # 返回
    /// 成功返回会话信息，失败返回错误
    pub fn open_session(
        &mut self,
        app: AppHandle,
        host: HostConfig,
    ) -> Result<SessionInfo, AppError> {
        // 生成唯一会话ID
        let session_id = Uuid::new_v4().to_string();
        // 创建会话信息
        let session_info = SessionInfo {
            session_id: session_id.clone(),
            host_id: host.id.clone(),
            host: host.host.clone(),
            port: host.port,
            username: host.username.clone(),
            status: SessionStatus::Connecting,
            created_at: chrono::Utc::now().timestamp(),
            active: true,
        };

        // 创建命令通道
        let (command_tx, command_rx) = mpsc::channel();
        // 创建关闭标志
        let shutdown = Arc::new(AtomicBool::new(false));

        // 将会话句柄存入管理器
        self.sessions.insert(
            session_id.clone(),
            SessionHandle {
                meta: session_info.clone(),
                command_tx,
                shutdown: shutdown.clone(),
                latest_status: None,
            },
        );

        // 发送"连接中"状态事件
        emit_session_status(&app, &session_id, SessionStatus::Connecting, None);

        // 启动终端工作线程（处理SSH连接和终端I/O）
        spawn_terminal_worker(
            app.clone(),
            host.clone(),
            session_id.clone(),
            command_rx,
            shutdown.clone(),
        );
        // 启动监控工作线程（定期收集服务器状态）
        spawn_monitor_worker(app, host, session_id, shutdown);

        Ok(session_info)
    }

    /// 向指定会话的终端写入数据
    pub fn write_terminal(&self, session_id: &str, data: String) -> Result<(), AppError> {
        let handle = self
            .sessions
            .get(session_id)
            .ok_or_else(|| AppError::SessionNotFound(session_id.to_string()))?;
        handle
            .command_tx
            .send(SessionCommand::Write(data))
            .map_err(|error| AppError::IoError(std::io::Error::other(error.to_string())))
    }

    /// 调整指定会话的终端大小
    pub fn resize_terminal(&self, session_id: &str, cols: u32, rows: u32) -> Result<(), AppError> {
        let handle = self
            .sessions
            .get(session_id)
            .ok_or_else(|| AppError::SessionNotFound(session_id.to_string()))?;
        handle
            .command_tx
            .send(SessionCommand::Resize { cols, rows })
            .map_err(|error| AppError::IoError(std::io::Error::other(error.to_string())))
    }

    /// 关闭指定会话
    pub fn close_session(&mut self, session_id: &str) -> Result<(), AppError> {
        let handle = self
            .sessions
            .remove(session_id)
            .ok_or_else(|| AppError::SessionNotFound(session_id.to_string()))?;
        // 设置关闭标志
        handle.shutdown.store(true, Ordering::Relaxed);
        // 发送关闭命令
        let _ = handle.command_tx.send(SessionCommand::Close);
        Ok(())
    }

    /// 获取所有会话列表
    pub fn list_sessions(&self) -> Vec<SessionInfo> {
        self.sessions
            .values()
            .map(|handle| handle.meta.clone())
            .collect()
    }

    /// 获取指定会话的最新监控状态
    pub fn get_monitor_status(&self, session_id: &str) -> Result<ServerStatus, AppError> {
        self.sessions
            .get(session_id)
            .and_then(|handle| handle.latest_status.clone())
            .ok_or_else(|| AppError::SessionNotFound(session_id.to_string()))
    }
}

/// 启动终端工作线程，处理SSH连接和终端I/O
///
/// # 参数
/// - `app`: Tauri应用句柄
/// - `host`: 主机配置
/// - `session_id`: 会话ID
/// - `command_rx`: 命令接收端
/// - `shutdown`: 关闭标志
fn spawn_terminal_worker(
    app: AppHandle,
    host: HostConfig,
    session_id: String,
    command_rx: Receiver<SessionCommand>,
    shutdown: Arc<AtomicBool>,
) {
    thread::spawn(move || {
        // 建立SSH连接
        let session = match ssh_client::connect(&host) {
            Ok(session) => session,
            Err(error) => {
                // 根据错误类型设置相应状态
                let status = if matches!(error, AppError::AuthenticationError(_)) {
                    SessionStatus::AuthFailed
                } else {
                    SessionStatus::Error
                };
                emit_session_status(&app, &session_id, status, Some(error.to_string()));
                return;
            }
        };

        // 设置为非阻塞模式
        session.set_blocking(false);
        // 创建SSH通道
        let mut channel = match session.channel_session() {
            Ok(channel) => channel,
            Err(error) => {
                emit_session_status(
                    &app,
                    &session_id,
                    SessionStatus::Error,
                    Some(error.to_string()),
                );
                return;
            }
        };

        // 请求PTY（伪终端）
        if let Err(error) = channel.request_pty("xterm", None, Some((120, 32, 0, 0))) {
            emit_session_status(
                &app,
                &session_id,
                SessionStatus::Error,
                Some(error.to_string()),
            );
            return;
        }

        // 启动Shell
        if let Err(error) = channel.shell() {
            emit_session_status(
                &app,
                &session_id,
                SessionStatus::Error,
                Some(error.to_string()),
            );
            return;
        }

        // 发送"已连接"状态事件
        emit_session_status(&app, &session_id, SessionStatus::Connected, None);

        // 读取缓冲区
        let mut buffer = [0_u8; 4096];

        // 主循环：读取终端输出和处理命令
        while !shutdown.load(Ordering::Relaxed) {
            // 读取终端输出
            match channel.read(&mut buffer) {
                Ok(size) if size > 0 => {
                    let data = String::from_utf8_lossy(&buffer[..size]).to_string();
                    let _ = app.emit(
                        "terminal:data",
                        TerminalDataEvent {
                            session_id: session_id.clone(),
                            data,
                        },
                    );
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(error) => {
                    emit_session_status(
                        &app,
                        &session_id,
                        SessionStatus::Disconnected,
                        Some(error.to_string()),
                    );
                    break;
                }
            }

            // 处理命令队列中的命令
            while let Ok(command) = command_rx.try_recv() {
                match command {
                    SessionCommand::Write(data) => {
                        if let Err(error) = terminal_bridge::write_channel(&mut channel, &data) {
                            emit_session_status(
                                &app,
                                &session_id,
                                SessionStatus::Error,
                                Some(error.to_string()),
                            );
                        }
                    }
                    SessionCommand::Resize { cols, rows } => {
                        if let Err(error) =
                            terminal_bridge::resize_channel(&mut channel, cols, rows)
                        {
                            emit_session_status(
                                &app,
                                &session_id,
                                SessionStatus::Error,
                                Some(error.to_string()),
                            );
                        }
                    }
                    SessionCommand::Close => {
                        let _ = channel.close();
                        emit_session_status(&app, &session_id, SessionStatus::Disconnected, None);
                        return;
                    }
                }
            }

            // 检查EOF（连接断开）
            if channel.eof() {
                emit_session_status(&app, &session_id, SessionStatus::Disconnected, None);
                break;
            }

            // 短暂休眠避免CPU占用过高
            thread::sleep(Duration::from_millis(30));
        }

        // 关闭通道
        let _ = channel.close();
    });
}

/// 启动监控工作线程，定期收集服务器状态
///
/// # 参数
/// - `app`: Tauri应用句柄
/// - `host`: 主机配置
/// - `session_id`: 会话ID
/// - `shutdown`: 关闭标志
fn spawn_monitor_worker(
    app: AppHandle,
    host: HostConfig,
    session_id: String,
    shutdown: Arc<AtomicBool>,
) {
    thread::spawn(move || {
        while !shutdown.load(Ordering::Relaxed) {
            // 收集服务器状态
            match monitor_worker::collect_status(&host, &session_id) {
                Ok(status) => {
                    let _ = app.emit("monitor:update", status);
                }
                Err(error) => {
                    emit_session_status(
                        &app,
                        &session_id,
                        SessionStatus::Error,
                        Some(error.to_string()),
                    );
                }
            }

            // 等待2秒（分20次100毫秒检查关闭标志）
            for _ in 0..20 {
                if shutdown.load(Ordering::Relaxed) {
                    return;
                }
                thread::sleep(Duration::from_millis(100));
            }
        }
    });
}

/// 发送会话状态变更事件到前端
///
/// # 参数
/// - `app`: Tauri应用句柄
/// - `session_id`: 会话ID
/// - `status`: 新的会话状态
/// - `message`: 可选的状态消息
fn emit_session_status(
    app: &AppHandle,
    session_id: &str,
    status: SessionStatus,
    message: Option<String>,
) {
    let _ = app.emit(
        "session:status",
        SessionStatusEvent {
            session_id: session_id.to_string(),
            status,
            message,
        },
    );
}
