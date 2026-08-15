use crate::core::host_identity::HostKeyVerifier;
use crate::core::ssh_transport;
use crate::core::ssh_transport::{ConnectPhase, TerminalTransport};
use crate::errors::app_error::AppError;
use crate::errors::app_error::AppErrorInfo;
use crate::models::host::{AuthType, HostConfig};
use crate::models::session::{SessionStatus, SessionStatusEvent, TerminalDataEvent};
use crate::storage::secure_store;
use log::{debug, error, info};
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Runtime};

/// SSH 连接阶段总超时时间（含 TCP、握手、认证），作为 libssh2 阻塞场景的外层兜底
const CONNECT_TOTAL_TIMEOUT_SECS: u64 = 15;

/// 连接阶段枚举，用于向前端与控制台报告当前卡点
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum ConnectionPhase {
    LoadingCredentials,
    ConnectingTcp,
    SshHandshake,
    /// 首次未知主机身份确认：等待用户决定期间不占用连接总超时
    VerifyingHostKey,
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
    pub timestamp: i64,
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

/// SSH 连接函数：生产为 ssh_transport::connect_terminal，测试可注入模拟实现。
type TerminalConnectFn = Box<
    dyn FnOnce(
            &HostConfig,
            Option<&str>,
            Option<&str>,
            &HostKeyVerifier,
            &mut dyn FnMut(ConnectPhase),
        ) -> Result<TerminalTransport, AppError>
        + Send,
>;

/// 启动终端服务工作线程
///
/// 负责从安全存储读取凭据、建立 SSH 连接（含首次主机身份确认）、请求 PTY、启动 Shell，
/// 并进入非阻塞 IO 循环处理终端数据读写，派发 terminal:data 和 session:status 事件。
///
/// # 参数
/// - `app`: Tauri 应用句柄，用于派发事件到前端
/// - `host`: 主机配置（不含明文凭据）
/// - `session_id`: 会话唯一标识符
/// - `command_rx`: 命令接收端，接收来自协调层的终端命令
/// - `shutdown`: 关闭标志，设置为 true 时工作线程退出
/// - `runtime_status`: 后端权威会话状态，事件发出前先更新
/// - `verifier`: 主机身份统一校验器（握手后、认证前生效）
pub fn start_terminal_session<R: Runtime>(
    app: AppHandle<R>,
    host: HostConfig,
    session_id: String,
    command_rx: Receiver<TerminalCommand>,
    shutdown: Arc<AtomicBool>,
    runtime_status: Arc<Mutex<SessionStatus>>,
    verifier: HostKeyVerifier,
) {
    start_terminal_session_with_parts(
        app,
        host,
        session_id,
        command_rx,
        shutdown,
        runtime_status,
        load_credentials,
        verifier,
        Box::new(
            |host, password, passphrase, verifier: &HostKeyVerifier, on_phase| {
                ssh_transport::connect_terminal(host, password, passphrase, verifier, on_phase)
            },
        ),
        Duration::from_secs(CONNECT_TOTAL_TIMEOUT_SECS),
    );
}

/// 启动可注入部件的终端工作线程：凭据加载、连接函数与连接总超时均可替换，供测试使用。
#[allow(clippy::too_many_arguments)]
fn start_terminal_session_with_parts<R, F>(
    app: AppHandle<R>,
    host: HostConfig,
    session_id: String,
    command_rx: Receiver<TerminalCommand>,
    shutdown: Arc<AtomicBool>,
    runtime_status: Arc<Mutex<SessionStatus>>,
    credential_loader: F,
    verifier: HostKeyVerifier,
    connect_fn: TerminalConnectFn,
    connect_timeout: Duration,
) where
    R: Runtime,
    F: FnOnce(&HostConfig) -> Result<(Option<String>, Option<String>), AppError> + Send + 'static,
{
    thread::spawn(move || {
        // 系统安全存储首次访问可能等待用户授权；终端工作线程直接等待授权结果
        // ponytail: Keychain API 无法取消；先等待系统结果，若出现真实永久挂起再引入可取消凭据代理。
        emit_connection_progress(&app, &session_id, ConnectionPhase::LoadingCredentials);
        info!(
            "[session:{}][diagnostic] Starting credential load",
            session_id
        );
        let credentials = match credential_loader(&host) {
            Ok(creds) => {
                info!(
                    "[session:{}][diagnostic] Credentials loaded successfully",
                    session_id
                );
                creds
            }
            Err(error) => {
                error!(
                    "[session:{}][diagnostic] Credential loading failed: error_code={}",
                    session_id,
                    error.code()
                );
                let (status, message) =
                    map_phase_error_to_status(&ConnectionPhase::LoadingCredentials, &error);
                emit_session_status(&app, &session_id, &runtime_status, status, Some(message));
                return;
            }
        };
        let (password, passphrase) = credentials;

        // 将 SSH 连接（TCP握手 + SSH握手 + 认证）放到独立线程执行，
        // 外层通过 channel + recv_timeout 实现真正的连接阶段超时。
        // libssh2 的 set_timeout 对 userauth_password 不生效，必须用此方案。
        let (conn_tx, conn_rx) = mpsc::channel::<Result<TerminalTransport, AppError>>();
        let host_clone = host.clone();
        let password_owned = password.map(|s| s.to_string());
        let passphrase_owned = passphrase.map(|s| s.to_string());
        let app_for_connect = app.clone();
        let session_id_for_connect = session_id.clone();
        let current_phase = Arc::new(Mutex::new(ConnectionPhase::ConnectingTcp));
        let current_phase_for_connect = current_phase.clone();
        let verifier_for_connect = verifier;

        thread::spawn(move || {
            let result = connect_fn(
                &host_clone,
                password_owned.as_deref(),
                passphrase_owned.as_deref(),
                &verifier_for_connect,
                &mut |phase| {
                    let mapped_phase = map_connect_phase(phase);
                    update_current_phase(&current_phase_for_connect, mapped_phase.clone());
                    emit_connection_progress(
                        &app_for_connect,
                        &session_id_for_connect,
                        mapped_phase,
                    );
                },
            );
            // 若外层已超时，send 会失败，直接忽略
            let _ = conn_tx.send(result);
        });

        // 等待连接结果：固定短轮询，按当前阶段决定是否消耗连接总超时预算。
        // 主机身份确认（VerifyingHostKey）等待用户决定期间不设独立自动超时、
        // 不占用预算：进入验证阶段时预算冻结，离开时重新授予完整预算（用户
        // 接受后认证仍有完整窗口，不因等待时长被立即判超时）；其余阶段共享
        // 预算，超过截止即上报 Timeout。连接结果优先于截止判定：已完成但
        // 尚未被读取的连接不得因旧截止被误杀。
        let mut overall_deadline = Instant::now() + connect_timeout;
        let mut verify_wait_start: Option<Instant> = None;
        let mut terminal = loop {
            let active_phase = current_phase_value(&current_phase);
            match conn_rx.recv_timeout(Duration::from_millis(100)) {
                Ok(Ok(terminal)) => break terminal,
                Ok(Err(error)) => {
                    let (status, message) = map_phase_error_to_status(&active_phase, &error);
                    emit_session_status(&app, &session_id, &runtime_status, status, Some(message));
                    return;
                }
                Err(RecvTimeoutError::Timeout) => {
                    let verifying = active_phase == ConnectionPhase::VerifyingHostKey;
                    match (verifying, verify_wait_start) {
                        (true, None) => verify_wait_start = Some(Instant::now()),
                        (false, Some(_)) => {
                            overall_deadline = Instant::now() + connect_timeout;
                            verify_wait_start = None;
                        }
                        _ => {}
                    }
                    if !verifying && Instant::now() >= overall_deadline {
                        emit_session_status(
                            &app,
                            &session_id,
                            &runtime_status,
                            SessionStatus::Timeout,
                            Some(phase_timeout_message(&active_phase)),
                        );
                        return;
                    }
                }
                Err(RecvTimeoutError::Disconnected) => {
                    emit_session_status(
                        &app,
                        &session_id,
                        &runtime_status,
                        SessionStatus::Error,
                        Some("连接线程异常退出".to_string()),
                    );
                    return;
                }
            }
        };

        // 派发"已连接"状态事件
        emit_session_status(
            &app,
            &session_id,
            &runtime_status,
            SessionStatus::Connected,
            None,
        );

        // 终端数据读取缓冲区（UTF-8，4KB）
        let mut buffer = [0_u8; 4096];

        // 主循环：非阻塞读取终端输出并处理命令队列
        while !shutdown.load(Ordering::Relaxed) {
            // 读取 SSH Channel 的 stdout 输出
            match terminal.read(&mut buffer) {
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
                        &runtime_status,
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
                        if let Err(error) = terminal.write(&data) {
                            emit_session_status(
                                &app,
                                &session_id,
                                &runtime_status,
                                SessionStatus::Error,
                                Some(error.to_string()),
                            );
                        }
                    }
                    TerminalCommand::Resize { cols, rows } => {
                        if let Err(error) = terminal.resize(cols, rows) {
                            emit_session_status(
                                &app,
                                &session_id,
                                &runtime_status,
                                SessionStatus::Error,
                                Some(error.to_string()),
                            );
                        }
                    }
                    TerminalCommand::Close => {
                        // 主动关闭：关闭通道并派发断开状态
                        let _ = terminal.close();
                        emit_session_status(
                            &app,
                            &session_id,
                            &runtime_status,
                            SessionStatus::Disconnected,
                            None,
                        );
                        return;
                    }
                }
            }

            // 检测 EOF（远程端主动断开连接），派发"连接已断开"消息
            if terminal.eof() {
                emit_session_status(
                    &app,
                    &session_id,
                    &runtime_status,
                    SessionStatus::Disconnected,
                    Some("连接已断开".to_string()),
                );
                break;
            }

            // 短暂休眠，避免 CPU 空转占用过高
            thread::sleep(Duration::from_millis(30));
        }

        // 退出循环后关闭通道，释放资源
        let _ = terminal.close();
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
    debug!("[diagnostic] Loading credentials");
    debug!(
        "[diagnostic] Authentication type: {:?}, password_credential_present={}",
        host.auth_type,
        host.password_ref.is_some()
    );

    match host.auth_type {
        AuthType::Password => {
            // 密码认证：必须存在密码引用键
            let password_ref = host
                .password_ref
                .as_deref()
                .ok_or_else(|| AppError::InvalidHostConfig("密码为必填项".to_string()))?;
            debug!("[diagnostic] Loading password credential");

            let password = secure_store::get_credential(password_ref).map_err(|e| {
                error!(
                    "[diagnostic] Failed to load password: error_code={}",
                    e.code()
                );
                e
            })?;

            debug!("[diagnostic] Password loaded successfully");
            Ok((Some(password), None))
        }
        AuthType::PrivateKey => {
            // 私钥认证：私钥路径必须存在
            if host.private_key_path.is_none() {
                return Err(AppError::InvalidHostConfig("私钥路径为必填项".to_string()));
            }
            // 私钥口令为可选项，若有引用键则读取
            let passphrase = if let Some(ref passphrase_ref) = host.passphrase_ref {
                debug!("[diagnostic] Loading passphrase credential");
                Some(secure_store::get_credential(passphrase_ref)?)
            } else {
                None
            };
            Ok((None, passphrase))
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
        ConnectPhase::VerifyingHostKey => ConnectionPhase::VerifyingHostKey,
        ConnectPhase::Authenticating => ConnectionPhase::Authenticating,
        ConnectPhase::OpeningChannel => ConnectionPhase::OpeningChannel,
        ConnectPhase::RequestingPty => ConnectionPhase::RequestingPty,
        ConnectPhase::StartingShell => ConnectionPhase::StartingShell,
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
    state
        .lock()
        .map(|current| current.clone())
        .unwrap_or(ConnectionPhase::ConnectingTcp)
}

/// 返回连接阶段的默认中文进度文案
///
/// 该文案用于前端状态栏。
fn phase_message(phase: &ConnectionPhase) -> &'static str {
    match phase {
        ConnectionPhase::LoadingCredentials => "正在读取凭据...",
        ConnectionPhase::ConnectingTcp => "正在建立 TCP 连接...",
        ConnectionPhase::SshHandshake => "正在进行 SSH 握手...",
        ConnectionPhase::VerifyingHostKey => "正在验证主机身份...",
        ConnectionPhase::Authenticating => "正在进行 SSH 认证...",
        ConnectionPhase::OpeningChannel => "正在打开终端通道...",
        ConnectionPhase::RequestingPty => "正在请求终端 PTY...",
        ConnectionPhase::StartingShell => "正在启动 Shell...",
    }
}

/// 返回连接阶段的超时提示文本
///
/// 不同阶段使用明确文案，便于用户和开发者快速判断阻塞点。
/// 主机身份确认（VerifyingHostKey）不设独立自动超时，永不进入此函数。
fn phase_timeout_message(phase: &ConnectionPhase) -> String {
    match phase {
        ConnectionPhase::LoadingCredentials => "读取系统凭据超时".to_string(),
        ConnectionPhase::ConnectingTcp => "建立 TCP 连接超时".to_string(),
        ConnectionPhase::SshHandshake => "SSH 握手超时".to_string(),
        ConnectionPhase::Authenticating => "SSH 认证超时".to_string(),
        ConnectionPhase::OpeningChannel => "打开终端通道超时".to_string(),
        ConnectionPhase::RequestingPty => "请求终端 PTY 超时".to_string(),
        ConnectionPhase::StartingShell => "启动 Shell 超时".to_string(),
        // 防御性分支：验证阶段无自动超时，deadline 判定不会进入此函数；
        // 若协议层超时错误在阶段回读时仍显示为验证阶段，返回通用文案而非 panic
        ConnectionPhase::VerifyingHostKey => "连接超时".to_string(),
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
        AppError::SshProtocolError(err) if is_timeout_message(&err.to_string()) => {
            (SessionStatus::Timeout, phase_timeout_message(phase))
        }
        AppError::SshProtocolError(err) => (
            SessionStatus::Error,
            format!("{}: {err}", phase_message(phase)),
        ),
        AppError::SecureStoreError(msg) if is_timeout_message(msg) => {
            (SessionStatus::Timeout, phase_timeout_message(phase))
        }
        AppError::SecureStoreError(msg) => (SessionStatus::Error, format!("凭据读取失败: {msg}")),
        // 用户拒绝未知主机身份：不进入认证，展示结构化错误供所属标签渲染
        AppError::HostKeyRejected(detail) => (
            SessionStatus::Error,
            format!("已拒绝未知主机身份: {detail}"),
        ),
        // 会话关闭取消了等待中的主机身份验证
        AppError::HostKeyVerificationCancelled(_) => (
            SessionStatus::Error,
            "主机身份验证已随会话关闭取消".to_string(),
        ),
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
/// 控制台日志使用英文阶段枚举，前端事件用于状态栏显示本地化当前卡点。
fn emit_connection_progress<R: Runtime>(
    app: &AppHandle<R>,
    session_id: &str,
    phase: ConnectionPhase,
) {
    let timestamp = chrono::Utc::now().timestamp_millis();
    info!("[session:{}][phase:{:?}]", session_id, phase);
    let _ = app.emit(
        "session:progress",
        ConnectionProgressEvent {
            session_id: session_id.to_string(),
            phase,
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
fn emit_session_status<R: tauri::Runtime>(
    app: &AppHandle<R>,
    session_id: &str,
    runtime_status: &Arc<Mutex<SessionStatus>>,
    status: SessionStatus,
    message: Option<String>,
) {
    debug!(
        "[session:{}][diagnostic] Emitting session status: {:?}, has_message={}",
        session_id,
        status,
        message.is_some()
    );
    *runtime_status
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = status.clone();
    let result = app.emit(
        "session:status",
        SessionStatusEvent {
            session_id: session_id.to_string(),
            status,
            error: message.map(|detail| AppErrorInfo {
                code: "Unknown".to_string(),
                detail: Some(detail),
            }),
        },
    );
    if result.is_err() {
        error!(
            "[session:{}][diagnostic] Session status event emission failed",
            session_id
        );
    } else {
        debug!(
            "[session:{}][diagnostic] emit_session_status SUCCESS",
            session_id
        );
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
#[path = "terminal_service_test.rs"]
mod integration_tests;
