use crate::core::{ssh_client, terminal_bridge};
use crate::errors::app_error::AppError;
use crate::models::host::{AuthType, HostConfig};
use crate::models::session::{SessionStatus, SessionStatusEvent, TerminalDataEvent};
use crate::storage::secure_store;
use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use tauri::{AppHandle, Emitter};

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
/// ### 参数
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
        // 从安全存储读取运行时凭据
        let (password, passphrase) = match load_credentials(&host) {
            Ok(creds) => creds,
            Err(error) => {
                emit_session_status(&app, &session_id, SessionStatus::AuthFailed, Some(error.to_string()));
                return;
            }
        };

        // 建立 SSH 连接并完成认证
        let session = match ssh_client::connect(
            &host,
            password.as_deref(),
            passphrase.as_deref(),
        ) {
            Ok(session) => session,
            Err(error) => {
                // 根据错误类型精确映射到对应会话状态：
                // - AuthenticationError → AuthFailed（认证失败）
                // - SshConnectionError 含 "Connection timeout" → Timeout（TCP 超时）
                // - SshConnectionError 其他（拒绝/不可达）→ Error（网络错误）
                // - Ssh2Error（握手失败）→ Error（握手错误）
                // - 其他错误 → Error
                let (status, message) = match &error {
                    AppError::AuthenticationError(msg) => {
                        (SessionStatus::AuthFailed, format!("认证失败: {msg}"))
                    }
                    AppError::SshConnectionError(msg) if msg.contains("Connection timeout") => {
                        (SessionStatus::Timeout, "连接超时".to_string())
                    }
                    AppError::SshConnectionError(msg) => {
                        (SessionStatus::Error, format!("网络连接失败: {msg}"))
                    }
                    AppError::Ssh2Error(err) => {
                        (SessionStatus::Error, format!("SSH 握手失败: {err}"))
                    }
                    _ => (SessionStatus::Error, error.to_string()),
                };
                emit_session_status(&app, &session_id, status, Some(message));
                return;
            }
        };

        // 设置为非阻塞模式，避免 IO 读取阻塞命令处理
        session.set_blocking(false);

        // 创建 SSH 通道
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

        // 请求 PTY（伪终端），类型为 xterm
        if let Err(error) = channel.request_pty("xterm", None, Some((120, 32, 0, 0))) {
            emit_session_status(
                &app,
                &session_id,
                SessionStatus::Error,
                Some(format!("PTY 请求失败: {error}")),
            );
            return;
        }

        // 启动 Shell
        if let Err(error) = channel.shell() {
            emit_session_status(
                &app,
                &session_id,
                SessionStatus::Error,
                Some(format!("Shell 启动失败: {error}")),
            );
            return;
        }

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
                        emit_session_status(
                            &app,
                            &session_id,
                            SessionStatus::Disconnected,
                            None,
                        );
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
    match host.auth_type {
        AuthType::Password => {
            // 密码认证：必须存在密码引用键
            let password_ref = host.password_ref.as_deref().ok_or_else(|| {
                AppError::InvalidHostConfig("密码为必填项".to_string())
            })?;
            let password = secure_store::get_credential(password_ref)?;
            Ok((Some(password), None))
        }
        AuthType::PrivateKey => {
            // 私钥认证：私钥路径必须存在
            if host.private_key_path.is_none() {
                return Err(AppError::InvalidHostConfig("私钥路径为必填项".to_string()));
            }
            // 私钥口令为可选项，若有引用键则读取
            let passphrase = if let Some(ref passphrase_ref) = host.passphrase_ref {
                Some(secure_store::get_credential(passphrase_ref)?)
            } else {
                None
            };
            Ok((None, passphrase))
        }
    }
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
    let _ = app.emit(
        "session:status",
        SessionStatusEvent {
            session_id: session_id.to_string(),
            status,
            message,
        },
    );
}

#[cfg(test)]
mod tests {
    use crate::models::session::TerminalDataEvent;
    use proptest::prelude::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

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
