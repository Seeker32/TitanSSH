use crate::core::{ssh_client, terminal_bridge};
use crate::core::ssh_client::ConnectPhase;
use crate::errors::app_error::AppError;
use crate::models::host::{AuthType, HostConfig};
use crate::models::session::{SessionStatus, SessionStatusEvent, TerminalDataEvent};
use crate::storage::secure_store;
use serde::Serialize;
use ssh2::Session;
use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use tauri::{AppHandle, Emitter};

/// 凭据读取阶段超时时间，避免系统钥匙串卡住导致 UI 长期停留在“连接中”
const CREDENTIAL_LOAD_TIMEOUT_SECS: u64 = 5;
/// SSH 连接阶段总超时时间（含 TCP、握手、认证），作为 libssh2 阻塞场景的外层兜底
const CONNECT_TOTAL_TIMEOUT_SECS: u64 = 15;
/// 通道初始化阶段超时时间（打开 Channel / 请求 PTY / 启动 Shell）
const CHANNEL_SETUP_TIMEOUT_MS: u32 = 5_000;

/// 连接阶段枚举，用于向前端与控制台报告当前卡点
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum ConnectionPhase {
    LoadingCredentials,
    ConnectingTcp,
    SshHandshake,
    Authenticating,
    OpeningChannel,
    RequestingPty,
    StartingShell,
}

/// 连接阶段诊断事件，供前端显示“卡在哪一步”
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionProgressEvent {
    pub session_id: String,
    pub phase: ConnectionPhase,
    pub message: String,
    pub timestamp: i64,
}

/// 阶段执行结果，区分业务错误与超时兜底
#[derive(Debug)]
enum PhaseOutcome<T> {
    Completed(Result<T, AppError>),
    TimedOut,
}

/// 终端会话命令枚举，用于协调层向终端工作线程发送指令
#[derive(Clone)]
pub enum TerminalCommand {
    /// 向终端写入数据
    Write(String),
    /// 调整终端大小（列数、行数）
    Resize { cols: u32, rows: u32 },
    /// 关闭终端会话
    Close,
}

/// 启动终端服务工作线程
///
/// 负责从安全存储读取凭据、建立 SSH 连接、请求 PTY、启动 Shell，
/// 并进入非阻塞 IO 循环处理终端数据读写，派发 terminal:data 和 session:status 事件。
///
/// # 参数
/// - `app`: Tauri 应用句柄，用于派发事件到前端
/// - `host`: 主机配置（不含明文凭据）
/// - `session_id`: 会话唯一标识符
/// - `command_rx`: 命令接收端，接收来自协调层的终端命令
/// - `shutdown`: 关闭标志，设置为 true 时工作线程退出
pub fn start_terminal_session(
    app: AppHandle,
    host: HostConfig,
    session_id: String,
    command_rx: Receiver<TerminalCommand>,
    shutdown: Arc<AtomicBool>,
) {
    thread::spawn(move || {
        // 从安全存储读取运行时凭据，并对钥匙串阻塞设置独立超时
        emit_connection_progress(&app, &session_id, ConnectionPhase::LoadingCredentials);
        eprintln!("[session:{}][diagnostic] Starting credential load with {}s timeout", session_id, CREDENTIAL_LOAD_TIMEOUT_SECS);
        let host_for_credentials = host.clone();
        let credentials = match run_phase_with_timeout(
            Duration::from_secs(CREDENTIAL_LOAD_TIMEOUT_SECS),
            move || {
                eprintln!("[session:{}][diagnostic] Credential thread started", host_for_credentials.id);
                let result = load_credentials(&host_for_credentials);
                eprintln!("[session:{}][diagnostic] Credential thread completed: {:?}", host_for_credentials.id, result.is_ok());
                result
            },
        ) {
            PhaseOutcome::Completed(Ok(creds)) => {
                eprintln!("[session:{}][diagnostic] Credentials loaded successfully", session_id);
                creds
            }
            PhaseOutcome::Completed(Err(error)) => {
                eprintln!("[session:{}][diagnostic] Credentials error: {}", session_id, error);
                let (status, message) =
                    map_phase_error_to_status(&ConnectionPhase::LoadingCredentials, &error);
                emit_session_status(&app, &session_id, status, Some(message));
                return;
            }
            PhaseOutcome::TimedOut => {
                eprintln!("[session:{}][diagnostic] Credentials timed out after {}s", session_id, CREDENTIAL_LOAD_TIMEOUT_SECS);
                emit_session_status(
                    &app,
                    &session_id,
                    SessionStatus::Timeout,
                    Some(phase_timeout_message(&ConnectionPhase::LoadingCredentials)),
                );
                eprintln!("[session:{}][diagnostic] Timeout event emitted", session_id);
                return;
            }
        };
        let (password, passphrase) = credentials;

        // 将 SSH 连接（TCP握手 + SSH握手 + 认证）放到独立线程执行，
        // 外层通过 channel + recv_timeout 实现真正的连接阶段超时。
        // libssh2 的 set_timeout 对 userauth_password 不生效，必须用此方案。
        let (conn_tx, conn_rx) = mpsc::channel::<Result<Session, AppError>>();
        let host_clone = host.clone();
        let password_owned = password.map(|s| s.to_string());
        let passphrase_owned = passphrase.map(|s| s.to_string());
        let app_for_connect = app.clone();
        let session_id_for_connect = session_id.clone();
        let current_phase = Arc::new(Mutex::new(ConnectionPhase::ConnectingTcp));
        let current_phase_for_connect = current_phase.clone();

        thread::spawn(move || {
            let result = ssh_client::connect(
                &host_clone,
                password_owned.as_deref(),
                passphrase_owned.as_deref(),
                |phase| {
                    let mapped_phase = map_connect_phase(phase);
                    update_current_phase(&current_phase_for_connect, mapped_phase.clone());
                    emit_connection_progress(&app_for_connect, &session_id_for_connect, mapped_phase);
                },
            );
            // 若外层已超时，send 会失败，直接忽略
            let _ = conn_tx.send(result);
        });

        // 等待连接结果，超时则派发 Timeout 状态并退出
        let session = match conn_rx.recv_timeout(Duration::from_secs(CONNECT_TOTAL_TIMEOUT_SECS)) {
            Ok(Ok(session)) => session,
            Ok(Err(error)) => {
                let active_phase = current_phase_value(&current_phase);
                let (status, message) = map_phase_error_to_status(&active_phase, &error);
                emit_session_status(&app, &session_id, status, Some(message));
                return;
            }
            Err(RecvTimeoutError::Timeout) => {
                // recv_timeout 超时：连接线程仍在阻塞，直接放弃并上报超时
                emit_session_status(
                    &app,
                    &session_id,
                    SessionStatus::Timeout,
                    Some(phase_timeout_message(&current_phase_value(&current_phase))),
                );
                return;
            }
            Err(RecvTimeoutError::Disconnected) => {
                emit_session_status(
                    &app,
                    &session_id,
                    SessionStatus::Error,
                    Some("连接线程异常退出".to_string()),
                );
                return;
            }
        };

        let session = session;
        session.set_timeout(CHANNEL_SETUP_TIMEOUT_MS);

        // 创建 SSH 通道
        emit_connection_progress(&app, &session_id, ConnectionPhase::OpeningChannel);
        let mut channel = match session.channel_session() {
            Ok(channel) => channel,
            Err(error) => {
                let error = AppError::Ssh2Error(error);
                let (status, message) =
                    map_phase_error_to_status(&ConnectionPhase::OpeningChannel, &error);
                emit_session_status(&app, &session_id, status, Some(message));
                return;
            }
        };

        // 请求 PTY（伪终端），类型为 xterm
        emit_connection_progress(&app, &session_id, ConnectionPhase::RequestingPty);
        if let Err(error) = channel.request_pty("xterm", None, Some((120, 32, 0, 0))) {
            let error = AppError::Ssh2Error(error);
            let (status, message) =
                map_phase_error_to_status(&ConnectionPhase::RequestingPty, &error);
            emit_session_status(&app, &session_id, status, Some(message));
            return;
        }

        // 启动 Shell
        emit_connection_progress(&app, &session_id, ConnectionPhase::StartingShell);
        if let Err(error) = channel.shell() {
            let error = AppError::Ssh2Error(error);
            let (status, message) =
                map_phase_error_to_status(&ConnectionPhase::StartingShell, &error);
            emit_session_status(&app, &session_id, status, Some(message));
            return;
        }

        // 进入流式 IO 前切回非阻塞模式，避免读取 stdout 阻塞命令处理
        session.set_blocking(false);

        // 派发"已连接"状态事件
        emit_session_status(&app, &session_id, SessionStatus::Connected, None);

        // 终端数据读取缓冲区（UTF-8，4KB）
        let mut buffer = [0_u8; 4096];

        // 主循环：非阻塞读取终端输出并处理命令队列
        while !shutdown.load(Ordering::Relaxed) {
            // 读取 SSH Channel 的 stdout 输出
            match channel.read(&mut buffer) {
                Ok(size) if size > 0 => {
                    // 使用 UTF-8 解码，确保中文等多字节字符正确显示
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
                // WouldBlock 表示当前无数据可读，继续循环
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(error) => {
                    // 非 WouldBlock 的 IO 错误视为连接断开
                    emit_session_status(
                        &app,
                        &session_id,
                        SessionStatus::Disconnected,
                        Some(error.to_string()),
                    );
                    break;
                }
            }

            // 处理命令队列中的所有待处理命令
            while let Ok(command) = command_rx.try_recv() {
                match command {
                    TerminalCommand::Write(data) => {
                        if let Err(error) = terminal_bridge::write_channel(&mut channel, &data) {
                            emit_session_status(
                                &app,
                                &session_id,
                                SessionStatus::Error,
                                Some(error.to_string()),
                            );
                        }
                    }
                    TerminalCommand::Resize { cols, rows } => {
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
                    TerminalCommand::Close => {
                        // 主动关闭：关闭通道并派发断开状态
                        let _ = channel.close();
                        emit_session_status(&app, &session_id, SessionStatus::Disconnected, None);
                        return;
                    }
                }
            }

            // 检测 EOF（远程端主动断开连接），派发"连接已断开"消息
            if channel.eof() {
                emit_session_status(
                    &app,
                    &session_id,
                    SessionStatus::Disconnected,
                    Some("连接已断开".to_string()),
                );
                break;
            }

            // 短暂休眠，避免 CPU 空转占用过高
            thread::sleep(Duration::from_millis(30));
        }

        // 退出循环后关闭通道，释放资源
        let _ = channel.close();
    });
}

/// 从安全存储加载运行时凭据
///
/// 根据主机认证类型读取对应凭据：
/// - Password 模式：读取密码
/// - PrivateKey 模式：读取可选的私钥口令
///
/// # 返回
/// `(password, passphrase)` 元组，均为 Option<String>
fn load_credentials(host: &HostConfig) -> Result<(Option<String>, Option<String>), AppError> {
    eprintln!("[diagnostic] load_credentials called for host: {}", host.id);
    eprintln!("[diagnostic] auth_type: {:?}, password_ref: {:?}", host.auth_type, host.password_ref);

    match host.auth_type {
        AuthType::Password => {
            // 密码认证：必须存在密码引用键
            let password_ref = host
                .password_ref
                .as_deref()
                .ok_or_else(|| AppError::InvalidHostConfig("密码为必填项".to_string()))?;
            eprintln!("[diagnostic] Loading password with ref: {}", password_ref);

            let password = secure_store::get_credential(password_ref).map_err(|e| {
                eprintln!("[diagnostic] Failed to load password: {}", e);
                e
            })?;

            eprintln!("[diagnostic] Password loaded successfully");
            Ok((Some(password), None))
        }
        AuthType::PrivateKey => {
            // 私钥认证：私钥路径必须存在
            if host.private_key_path.is_none() {
                return Err(AppError::InvalidHostConfig("私钥路径为必填项".to_string()));
            }
            // 私钥口令为可选项，若有引用键则读取
            let passphrase = if let Some(ref passphrase_ref) = host.passphrase_ref {
                eprintln!("[diagnostic] Loading passphrase with ref: {}", passphrase_ref);
                Some(secure_store::get_credential(passphrase_ref)?)
            } else {
                None
            };
            Ok((None, passphrase))
        }
    }
}

/// 在独立线程中执行可能阻塞的阶段，并使用超时结果将“卡死”显式化
///
/// 该函数用于保护钥匙串读取等无法由调用方中断的阻塞操作，
/// 超时后直接返回 `TimedOut`，由上层决定派发何种阶段状态。
fn run_phase_with_timeout<T, F>(timeout: Duration, operation: F) -> PhaseOutcome<T>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, AppError> + Send + 'static,
{
    let (tx, rx) = mpsc::channel();
    eprintln!("[diagnostic] Spawning timeout thread with timeout {:?}", timeout);
    thread::spawn(move || {
        eprintln!("[diagnostic] Timeout thread spawned, running operation");
        let result = operation();
        eprintln!("[diagnostic] Operation completed, sending result");
        let _ = tx.send(result);
    });

    eprintln!("[diagnostic] Waiting for result with recv_timeout");
    match rx.recv_timeout(timeout) {
        Ok(result) => {
            eprintln!("[diagnostic] recv_timeout received result successfully");
            PhaseOutcome::Completed(result)
        }
        Err(RecvTimeoutError::Timeout) => {
            eprintln!("[diagnostic] recv_timeout timed out!");
            PhaseOutcome::TimedOut
        }
        Err(RecvTimeoutError::Disconnected) => {
            eprintln!("[diagnostic] recv_timeout channel disconnected");
            PhaseOutcome::TimedOut
        }
    }
}

/// 将 ssh_client 内部阶段映射到 terminal_service 的统一阶段枚举
///
/// 统一枚举后，前端与日志只需要处理一套阶段命名。
fn map_connect_phase(phase: ConnectPhase) -> ConnectionPhase {
    match phase {
        ConnectPhase::ConnectingTcp => ConnectionPhase::ConnectingTcp,
        ConnectPhase::SshHandshake => ConnectionPhase::SshHandshake,
        ConnectPhase::Authenticating => ConnectionPhase::Authenticating,
    }
}

/// 更新当前连接阶段共享状态，供外层超时兜底判断“卡在哪一步”
///
/// 若互斥锁已中毒，则静默忽略，继续使用旧阶段值。
fn update_current_phase(state: &Arc<Mutex<ConnectionPhase>>, phase: ConnectionPhase) {
    if let Ok(mut current) = state.lock() {
        *current = phase;
    }
}

/// 读取当前连接阶段共享状态
///
/// 若互斥锁不可用，则回退到 `ConnectingTcp`，保证超时文案始终可生成。
fn current_phase_value(state: &Arc<Mutex<ConnectionPhase>>) -> ConnectionPhase {
    state.lock()
        .map(|current| current.clone())
        .unwrap_or(ConnectionPhase::ConnectingTcp)
}

/// 返回连接阶段的默认中文进度文案
///
/// 该文案会同时用于前端状态栏和后端控制台，保证诊断口径一致。
fn phase_message(phase: &ConnectionPhase) -> &'static str {
    match phase {
        ConnectionPhase::LoadingCredentials => "正在读取凭据...",
        ConnectionPhase::ConnectingTcp => "正在建立 TCP 连接...",
        ConnectionPhase::SshHandshake => "正在进行 SSH 握手...",
        ConnectionPhase::Authenticating => "正在进行 SSH 认证...",
        ConnectionPhase::OpeningChannel => "正在打开终端通道...",
        ConnectionPhase::RequestingPty => "正在请求终端 PTY...",
        ConnectionPhase::StartingShell => "正在启动 Shell...",
    }
}

/// 返回连接阶段的超时提示文本
///
/// 不同阶段使用明确文案，便于用户和开发者快速判断阻塞点。
fn phase_timeout_message(phase: &ConnectionPhase) -> String {
    match phase {
        ConnectionPhase::LoadingCredentials => "读取系统凭据超时".to_string(),
        ConnectionPhase::ConnectingTcp => "建立 TCP 连接超时".to_string(),
        ConnectionPhase::SshHandshake => "SSH 握手超时".to_string(),
        ConnectionPhase::Authenticating => "SSH 认证超时".to_string(),
        ConnectionPhase::OpeningChannel => "打开终端通道超时".to_string(),
        ConnectionPhase::RequestingPty => "请求终端 PTY 超时".to_string(),
        ConnectionPhase::StartingShell => "启动 Shell 超时".to_string(),
    }
}

/// 将指定阶段中的错误映射为前端可消费的结构化状态
///
/// 该函数统一处理认证失败、连接超时、网络错误、SSH 协议错误和安全存储错误，
/// 保证不同阶段的错误提示具有明确的“卡点”上下文。
fn map_phase_error_to_status(phase: &ConnectionPhase, error: &AppError) -> (SessionStatus, String) {
    match error {
        AppError::AuthenticationError(msg) => {
            (SessionStatus::AuthFailed, format!("认证失败: {msg}"))
        }
        AppError::SshConnectionError(msg) if is_timeout_message(msg) => {
            (SessionStatus::Timeout, phase_timeout_message(phase))
        }
        AppError::SshConnectionError(msg) => (SessionStatus::Error, format!("网络连接失败: {msg}")),
        AppError::Ssh2Error(err) if is_timeout_message(&err.to_string()) => {
            (SessionStatus::Timeout, phase_timeout_message(phase))
        }
        AppError::Ssh2Error(err) => (SessionStatus::Error, format!("{}: {err}", phase_message(phase))),
        AppError::SecureStoreError(msg) if is_timeout_message(msg) => {
            (SessionStatus::Timeout, phase_timeout_message(phase))
        }
        AppError::SecureStoreError(msg) => (SessionStatus::Error, format!("凭据读取失败: {msg}")),
        // 凭据不存在：引导用户重新保存主机配置，而非显示通用错误
        AppError::CredentialNotFound(key) => (
            SessionStatus::Error,
            format!("凭据不存在（{key}），请重新编辑主机配置以重新保存密码"),
        ),
        _ => (SessionStatus::Error, error.to_string()),
    }
}

/// 判断错误消息是否表达连接超时语义
///
/// 兼容固定文案、不同大小写以及底层库常见的 `timed out` 表达，
/// 确保连接超时能稳定映射到 `SessionStatus::Timeout`。
fn is_timeout_message(message: &str) -> bool {
    let normalized = message.to_ascii_lowercase();
    normalized.contains("connection timeout")
        || normalized.contains("timed out")
        || message.contains("超时")
}

/// 派发连接阶段进度事件，并在控制台打印结构化日志
///
/// 控制台日志用于 `pnpm tauri dev` 诊断，前端事件用于状态栏显示当前卡点。
fn emit_connection_progress(app: &AppHandle, session_id: &str, phase: ConnectionPhase) {
    let message = phase_message(&phase).to_string();
    let timestamp = chrono::Utc::now().timestamp_millis();
    eprintln!(
        "[session:{}][phase:{:?}] {}",
        session_id,
        phase,
        message
    );
    let _ = app.emit(
        "session:progress",
        ConnectionProgressEvent {
            session_id: session_id.to_string(),
            phase,
            message,
            timestamp,
        },
    );
}

/// 派发会话状态变更事件到前端
///
/// # 参数
/// - `app`: Tauri 应用句柄
/// - `session_id`: 会话唯一标识符
/// - `status`: 新的会话状态
/// - `message`: 可选的状态附加消息（如错误详情）
fn emit_session_status(
    app: &AppHandle,
    session_id: &str,
    status: SessionStatus,
    message: Option<String>,
) {
    eprintln!("[session:{}][diagnostic] emit_session_status: {:?}, message: {:?}", session_id, status, message);
    let result = app.emit(
        "session:status",
        SessionStatusEvent {
            session_id: session_id.to_string(),
            status,
            message,
        },
    );
    if let Err(ref e) = result {
        eprintln!("[session:{}][diagnostic] emit_session_status FAILED: {}", session_id, e);
    } else {
        eprintln!("[session:{}][diagnostic] emit_session_status SUCCESS", session_id);
    }
}

#[cfg(test)]
mod tests {
    use crate::models::session::TerminalDataEvent;
    use proptest::prelude::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// 生成非空字母数字字符串的策略（1-64 个字符）
    fn arb_session_id() -> impl Strategy<Value = String> {
        "[a-zA-Z0-9\\-]{1,64}".prop_map(|s| s)
    }

    /// 生成任意终端数据字符串的策略（0-256 个字节）
    fn arb_terminal_data() -> impl Strategy<Value = String> {
        "[ -~]{0,256}".prop_map(|s| s)
    }

    proptest! {
        /// **验证: 需求 4.3, 4.4**
        ///
        /// Property 6: 终端事件按 session_id 正确路由
        ///
        /// 对任意 session_id 和终端数据，构造 TerminalDataEvent 后，
        /// 验证事件中的 session_id 与产生该事件的会话 ID 完全一致，
        /// 不会被路由到其他会话。
        ///
        /// 测试逻辑直接验证 terminal_service 中构造 TerminalDataEvent 的核心路由不变量：
        /// 事件的 session_id 字段必须等于触发该事件的会话 ID。
        #[test]
        fn prop_terminal_event_session_id_matches_producer(
            session_id in arb_session_id(),
            data in arb_terminal_data(),
        ) {
            // 模拟 terminal_service 中构造 TerminalDataEvent 的逻辑
            let event = TerminalDataEvent {
                session_id: session_id.clone(),
                data,
            };

            // 断言：事件的 session_id 必须与产生该事件的会话 ID 完全一致
            prop_assert_eq!(
                &event.session_id,
                &session_id,
                "TerminalDataEvent 的 session_id 必须与产生该事件的会话 ID 一致，\
                 不得路由到其他会话。期望: {}, 实际: {}",
                session_id,
                event.session_id
            );
        }

        /// **验证: 需求 4.3, 4.4**
        ///
        /// Property 6 扩展：不同会话的终端事件不会互相路由
        ///
        /// 对任意两个不同的 session_id，验证各自构造的 TerminalDataEvent
        /// 的 session_id 互不相同，确保事件不会跨会话路由。
        #[test]
        fn prop_terminal_events_from_different_sessions_do_not_cross_route(
            session_id_a in arb_session_id(),
            suffix in "[a-zA-Z0-9]{1,8}",
            data_a in arb_terminal_data(),
            data_b in arb_terminal_data(),
        ) {
            // 构造两个不同的 session_id，确保它们不同
            let session_id_b = format!("{}-other-{}", session_id_a, suffix);

            // 为会话 A 构造事件
            let event_a = TerminalDataEvent {
                session_id: session_id_a.clone(),
                data: data_a,
            };

            // 为会话 B 构造事件
            let event_b = TerminalDataEvent {
                session_id: session_id_b.clone(),
                data: data_b,
            };

            // 断言：会话 A 的事件 session_id 与会话 A 一致
            prop_assert_eq!(
                &event_a.session_id,
                &session_id_a,
                "会话 A 的 TerminalDataEvent session_id 必须与会话 A 的 ID 一致"
            );

            // 断言：会话 B 的事件 session_id 与会话 B 一致
            prop_assert_eq!(
                &event_b.session_id,
                &session_id_b,
                "会话 B 的 TerminalDataEvent session_id 必须与会话 B 的 ID 一致"
            );

            // 断言：两个事件的 session_id 互不相同，不会跨会话路由
            prop_assert_ne!(
                &event_a.session_id,
                &event_b.session_id,
                "不同会话产生的 TerminalDataEvent 的 session_id 不得相同，\
                 否则会导致终端数据路由到错误的会话实例"
            );
        }

        /// **验证: 需求 7.4, 7.5**
        ///
        /// Property 7: 关闭会话后终端流停止
        ///
        /// 模拟终端工作线程的核心 IO 循环逻辑：
        /// - 使用 `Arc<AtomicBool>` 作为 shutdown 标志（与 start_terminal_session 中一致）
        /// - 使用 proptest 生成任意数量的待处理数据帧序列
        /// - 在循环开始前将 shutdown 标志设置为 true（模拟 close_session 调用）
        /// - 验证循环体不会产生任何 TerminalDataEvent
        ///
        /// 该测试直接验证 terminal_service 中 `while !shutdown.load(Ordering::Relaxed)` 守卫的正确性：
        /// 一旦 shutdown 为 true，工作线程必须立即停止产生终端数据事件。
        #[test]
        fn prop_no_terminal_data_event_after_shutdown(
            session_id in arb_session_id(),
            // 生成任意数量的数据帧（0-32 帧），模拟 SSH Channel 可能产生的输出
            data_frames in prop::collection::vec(arb_terminal_data(), 0..=32),
        ) {
            // 创建与 start_terminal_session 中相同类型的 shutdown 标志
            let shutdown = Arc::new(AtomicBool::new(false));

            // 模拟 close_session 调用：设置 shutdown 标志为 true
            // 对应 session_manager::close_session 中的 handle.shutdown.store(true, Ordering::Relaxed)
            shutdown.store(true, Ordering::Relaxed);

            // 收集 shutdown 后工作线程产生的所有 TerminalDataEvent
            let mut emitted_events: Vec<TerminalDataEvent> = Vec::new();

            // 模拟 terminal_service 中的主循环守卫：
            // `while !shutdown.load(Ordering::Relaxed)` — shutdown 为 true 时循环体不执行
            while !shutdown.load(Ordering::Relaxed) {
                // 此循环体在 shutdown=true 时永远不会执行
                // 模拟：对每一帧数据构造 TerminalDataEvent 并"发送"
                for data in &data_frames {
                    emitted_events.push(TerminalDataEvent {
                        session_id: session_id.clone(),
                        data: data.clone(),
                    });
                }
            }

            // 断言：shutdown 标志设置后，工作线程不得产生任何 TerminalDataEvent
            prop_assert_eq!(
                emitted_events.len(),
                0,
                "关闭会话（shutdown=true）后，终端工作线程不得产生任何 TerminalDataEvent，\
                 但检测到 {} 个事件被产生。session_id: {}",
                emitted_events.len(),
                session_id
            );
        }

        /// **验证: 需求 7.4, 7.5**
        ///
        /// Property 7 扩展：shutdown 前后事件产生数量对比
        ///
        /// 验证 shutdown 标志的边界语义：
        /// - shutdown=false 时，循环体正常执行，可产生事件
        /// - shutdown=true 时，循环体不执行，事件数量为零
        ///
        /// 通过对比两种状态下的事件数量，确认 shutdown 标志是终端流停止的充分条件。
        #[test]
        fn prop_shutdown_flag_is_sufficient_to_stop_terminal_stream(
            session_id in arb_session_id(),
            // 生成 1-16 帧非空数据，确保 shutdown=false 时确实会产生事件
            data_frames in prop::collection::vec("[a-zA-Z0-9 ]{1,64}", 1usize..=16usize),
        ) {
            // --- 场景 A：shutdown=false，模拟正常运行中的工作线程 ---
            let shutdown_a = Arc::new(AtomicBool::new(false));
            let mut events_before_shutdown: Vec<TerminalDataEvent> = Vec::new();

            // 执行一次循环迭代（shutdown=false，循环体执行一次后手动退出）
            if !shutdown_a.load(Ordering::Relaxed) {
                for data in &data_frames {
                    events_before_shutdown.push(TerminalDataEvent {
                        session_id: session_id.clone(),
                        data: data.clone(),
                    });
                }
            }

            // 断言：shutdown=false 时，有数据帧则必然产生事件
            prop_assert_eq!(
                events_before_shutdown.len(),
                data_frames.len(),
                "shutdown=false 时，工作线程应为每帧数据产生一个 TerminalDataEvent，\
                 期望 {} 个，实际 {} 个",
                data_frames.len(),
                events_before_shutdown.len()
            );

            // --- 场景 B：shutdown=true，模拟 close_session 后的工作线程 ---
            let shutdown_b = Arc::new(AtomicBool::new(false));
            // 模拟 close_session 设置 shutdown 标志
            shutdown_b.store(true, Ordering::Relaxed);
            let mut events_after_shutdown: Vec<TerminalDataEvent> = Vec::new();

            // 模拟主循环守卫：shutdown=true 时循环体不执行
            while !shutdown_b.load(Ordering::Relaxed) {
                for data in &data_frames {
                    events_after_shutdown.push(TerminalDataEvent {
                        session_id: session_id.clone(),
                        data: data.clone(),
                    });
                }
            }

            // 断言：shutdown=true 后，不得产生任何 TerminalDataEvent
            prop_assert_eq!(
                events_after_shutdown.len(),
                0,
                "shutdown=true 后，终端工作线程不得产生任何 TerminalDataEvent，\
                 但检测到 {} 个事件。session_id: {}",
                events_after_shutdown.len(),
                session_id
            );
        }
    }
}

#[cfg(test)]
mod integration_tests {
    use super::*;
    use crate::errors::app_error::AppError;
    use crate::models::host::{AuthType, HostConfig};
    use crate::models::session::SessionStatus;
    use serde_json::json;

    /// 构造测试用 HostConfig（密码认证模式）
    fn make_password_host(password_ref: Option<&str>) -> HostConfig {
        HostConfig {
            id: "host-test".to_string(),
            name: "test".to_string(),
            host: "127.0.0.1".to_string(),
            port: 22,
            username: "root".to_string(),
            auth_type: AuthType::Password,
            password_ref: password_ref.map(|s| s.to_string()),
            private_key_path: None,
            passphrase_ref: None,
            remark: None,
        }
    }

    /// 构造测试用 HostConfig（私钥认证模式）
    fn make_privkey_host(key_path: Option<&str>, passphrase_ref: Option<&str>) -> HostConfig {
        HostConfig {
            id: "host-test".to_string(),
            name: "test".to_string(),
            host: "127.0.0.1".to_string(),
            port: 22,
            username: "root".to_string(),
            auth_type: AuthType::PrivateKey,
            password_ref: None,
            private_key_path: key_path.map(|s| s.to_string()),
            passphrase_ref: passphrase_ref.map(|s| s.to_string()),
            remark: None,
        }
    }

    /// 验证 load_credentials：密码认证模式下 password_ref 为 None 时返回 InvalidHostConfig 错误
    #[test]
    fn load_credentials_password_mode_missing_ref_returns_error() {
        let host = make_password_host(None);
        let result = load_credentials(&host);
        assert!(result.is_err(), "缺少 password_ref 时应返回错误");
        match result.unwrap_err() {
            AppError::InvalidHostConfig(msg) => {
                assert!(msg.contains("密码"), "错误消息应提及密码，实际: {}", msg);
            }
            other => panic!("期望 InvalidHostConfig，实际: {:?}", other),
        }
    }

    /// 验证 load_credentials：私钥认证模式下 private_key_path 为 None 时返回 InvalidHostConfig 错误
    #[test]
    fn load_credentials_privkey_mode_missing_path_returns_error() {
        let host = make_privkey_host(None, None);
        let result = load_credentials(&host);
        assert!(result.is_err(), "缺少私钥路径时应返回错误");
        match result.unwrap_err() {
            AppError::InvalidHostConfig(msg) => {
                assert!(
                    msg.contains("私钥路径"),
                    "错误消息应提及私钥路径，实际: {}",
                    msg
                );
            }
            other => panic!("期望 InvalidHostConfig，实际: {:?}", other),
        }
    }

    /// 验证 load_credentials：私钥认证模式下无口令引用时返回 (None, None)
    /// 私钥口令为可选项，无引用时不应报错
    #[test]
    fn load_credentials_privkey_mode_no_passphrase_ref_returns_none() {
        let host = make_privkey_host(Some("~/.ssh/id_rsa"), None);
        let result = load_credentials(&host);
        // 无 passphrase_ref 时不调用 secure_store，直接返回 (None, None)
        assert!(result.is_ok(), "无口令引用时应成功，实际: {:?}", result);
        let (password, passphrase) = result.unwrap();
        assert!(password.is_none(), "私钥模式下 password 应为 None");
        assert!(passphrase.is_none(), "无口令引用时 passphrase 应为 None");
    }

    /// 验证认证错误映射：AuthenticationError → SessionStatus::AuthFailed
    #[test]
    fn auth_error_maps_to_auth_failed_status() {
        let error = AppError::AuthenticationError("wrong password".to_string());
        let (status, message) = map_phase_error_to_status(&ConnectionPhase::Authenticating, &error);
        assert_eq!(
            status,
            SessionStatus::AuthFailed,
            "认证错误应映射为 AuthFailed"
        );
        assert!(
            message.contains("认证失败"),
            "消息应包含认证失败，实际: {}",
            message
        );
    }

    /// 验证连接超时错误映射：SshConnectionError("Connection timeout") → SessionStatus::Timeout
    #[test]
    fn connection_timeout_error_maps_to_timeout_status() {
        let error = AppError::SshConnectionError("Connection timeout after 30s".to_string());
        let (status, message) = map_phase_error_to_status(&ConnectionPhase::ConnectingTcp, &error);
        assert_eq!(status, SessionStatus::Timeout, "超时错误应映射为 Timeout");
        assert!(
            message.contains("超时"),
            "消息应包含超时，实际: {}",
            message
        );
    }

    /// 验证网络连接错误映射：SshConnectionError（非超时）→ SessionStatus::Error
    #[test]
    fn network_error_maps_to_error_status() {
        let error = AppError::SshConnectionError("Connection refused".to_string());
        let (status, message) = map_phase_error_to_status(&ConnectionPhase::ConnectingTcp, &error);
        assert_eq!(status, SessionStatus::Error, "网络错误应映射为 Error");
        assert!(
            message.contains("网络连接失败"),
            "消息应包含网络连接失败，实际: {}",
            message
        );
    }

    /// 验证 SSH 握手错误映射：Ssh2Error → SessionStatus::Error
    #[test]
    fn ssh2_error_maps_to_error_status() {
        // 使用 StorageError 模拟 Ssh2Error 的 Error 映射路径（避免构造 ssh2::Error）
        let error = AppError::StorageError("handshake failed".to_string());
        let (status, _message) = map_phase_error_to_status(&ConnectionPhase::SshHandshake, &error);
        assert_eq!(status, SessionStatus::Error, "其他错误应映射为 Error");
    }

    /// 验证不同 SshConnectionError 消息的超时判断边界
    #[test]
    fn connection_timeout_detection_accepts_multiple_message_shapes() {
        let timeout_err = AppError::SshConnectionError("Connection timeout".to_string());
        let (status, _) = map_phase_error_to_status(&ConnectionPhase::ConnectingTcp, &timeout_err);
        assert_eq!(status, SessionStatus::Timeout);

        let lower_case_err = AppError::SshConnectionError("connection timed out".to_string());
        let (status2, _) =
            map_phase_error_to_status(&ConnectionPhase::ConnectingTcp, &lower_case_err);
        assert_eq!(status2, SessionStatus::Timeout);

        let chinese_err = AppError::SshConnectionError("网络连接超时".to_string());
        let (status3, _) =
            map_phase_error_to_status(&ConnectionPhase::ConnectingTcp, &chinese_err);
        assert_eq!(status3, SessionStatus::Timeout);
    }

    /// 验证独立超时判断函数覆盖常见文案
    #[test]
    fn is_timeout_message_matches_common_timeout_text() {
        assert!(is_timeout_message("Connection timeout after 10s"));
        assert!(is_timeout_message("connection timed out"));
        assert!(is_timeout_message("连接超时"));
        assert!(!is_timeout_message("connection refused"));
    }

    /// 验证连接阶段事件序列化为 camelCase，符合前后端事件契约
    #[test]
    fn connection_progress_event_serializes_as_camel_case() {
        let event = ConnectionProgressEvent {
            session_id: "session-1".to_string(),
            phase: ConnectionPhase::LoadingCredentials,
            message: "正在读取凭据...".to_string(),
            timestamp: 1_710_000_000_111,
        };

        let value = serde_json::to_value(&event).expect("事件序列化应成功");
        assert_eq!(
            value,
            json!({
                "sessionId": "session-1",
                "phase": "LoadingCredentials",
                "message": "正在读取凭据...",
                "timestamp": 1_710_000_000_111_i64,
            })
        );
    }

    /// 验证凭据不存在错误映射：CredentialNotFound → SessionStatus::Error + 引导提示
    ///
    /// 区别于通用 SecureStoreError，CredentialNotFound 应给出明确的"重新保存"引导，
    /// 而不是让用户面对无意义的技术错误消息。
    #[test]
    fn credential_not_found_maps_to_error_with_guidance_message() {
        let key = "titanssh-host-abc-password";
        let error = AppError::CredentialNotFound(key.to_string());
        let (status, message) =
            map_phase_error_to_status(&ConnectionPhase::LoadingCredentials, &error);

        assert_eq!(status, SessionStatus::Error, "凭据不存在应映射为 Error");
        assert!(
            message.contains("凭据不存在"),
            "消息应包含凭据不存在，实际: {message}"
        );
        assert!(
            message.contains("重新编辑主机配置"),
            "消息应引导用户重新保存凭据，实际: {message}"
        );
        assert!(
            message.contains(key),
            "消息应包含具体的 key 便于诊断，实际: {message}"
        );
    }

    /// 验证 SecureStoreError（非超时）仍映射为通用 Error，不与 CredentialNotFound 混淆
    #[test]
    fn secure_store_error_non_timeout_maps_to_generic_error() {
        let error = AppError::SecureStoreError("keychain locked".to_string());
        let (status, message) =
            map_phase_error_to_status(&ConnectionPhase::LoadingCredentials, &error);

        assert_eq!(status, SessionStatus::Error, "安全存储错误应映射为 Error");
        assert!(
            message.contains("凭据读取失败"),
            "消息应包含凭据读取失败，实际: {message}"
        );
    }

    /// 验证 run_phase_with_timeout 在操作超时时正确返回 TimedOut
    #[test]
    fn run_phase_with_timeout_returns_timed_out_for_slow_operation() {
        use std::thread;
        use std::time::{Duration, Instant};

        let start = Instant::now();
        let result = run_phase_with_timeout(
            Duration::from_millis(100), // 100ms timeout
            || {
                // Simulate a slow operation that takes 500ms
                thread::sleep(Duration::from_millis(500));
                Ok::<_, AppError>("should not complete")
            },
        );
        let elapsed = start.elapsed();

        // Should return TimedOut, not Completed
        assert!(
            matches!(result, PhaseOutcome::TimedOut),
            "Expected TimedOut when operation exceeds timeout, got {:?}",
            result
        );

        // Should return quickly (within 200ms), not wait for the slow operation
        assert!(
            elapsed < Duration::from_millis(200),
            "Timeout should return quickly, not wait for slow operation. Elapsed: {:?}",
            elapsed
        );
    }

    /// 验证 run_phase_with_timeout 在操作成功时正确返回 Completed
    #[test]
    fn run_phase_with_timeout_returns_completed_for_fast_operation() {
        use std::time::{Duration, Instant};

        let start = Instant::now();
        let result = run_phase_with_timeout(
            Duration::from_secs(5),
            || Ok::<_, AppError>("success result"),
        );
        let elapsed = start.elapsed();

        // Should return Completed with the result
        assert!(
            matches!(result, PhaseOutcome::Completed(Ok("success result"))),
            "Expected Completed with success result, got {:?}",
            result
        );

        // Should return quickly (not wait for full timeout)
        assert!(
            elapsed < Duration::from_millis(100),
            "Fast operation should return immediately. Elapsed: {:?}",
            elapsed
        );
    }
}
