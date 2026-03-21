use crate::errors::app_error::AppError;
use crate::models::host::{AuthType, HostConfig};
use serde::Serialize;
use ssh2::Session;
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::path::Path;
use std::time::Duration;

/// SSH TCP 建连固定超时时间，避免前端长期停留在“连接中”状态
const SSH_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// SSH 握手与认证阶段超时时间，单位毫秒
const SSH_PROTOCOL_TIMEOUT_MS: u32 = 10_000;

/// SSH 建连内部阶段，用于向上层报告当前卡点
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum ConnectPhase {
    ConnectingTcp,
    SshHandshake,
    Authenticating,
}

/// 建立 SSH 连接并完成认证，返回已认证的 Session
///
/// # 参数
/// - `host`: 主机配置（不含明文凭据）
/// - `password`: 运行时从安全存储读取的明文密码（Password 认证时必须提供）
/// - `passphrase`: 运行时从安全存储读取的明文私钥口令（PrivateKey 认证时可选）
/// - `on_phase`: 阶段回调，供上层发出诊断事件与日志
///
/// # 返回
/// 成功返回已认证的 ssh2::Session，失败返回对应的 AppError
pub fn connect<F>(
    host: &HostConfig,
    password: Option<&str>,
    passphrase: Option<&str>,
    mut on_phase: F,
) -> Result<Session, AppError>
where
    F: FnMut(ConnectPhase),
{
    // 建立带固定超时的 TCP 连接，避免在网络黑洞场景下无限等待
    on_phase(ConnectPhase::ConnectingTcp);
    let socket_addrs = resolve_socket_addrs(host)?;
    let tcp = connect_tcp_stream(&socket_addrs, SSH_CONNECT_TIMEOUT)?;

    // 握手阶段需要较长的读写超时，避免网络抖动导致假超时
    tcp.set_read_timeout(Some(Duration::from_millis(SSH_PROTOCOL_TIMEOUT_MS.into())))?;
    tcp.set_write_timeout(Some(Duration::from_millis(SSH_PROTOCOL_TIMEOUT_MS.into())))?;

    // 创建 SSH 会话并完成握手
    // set_timeout 覆盖 handshake / userauth 等所有阻塞操作，单位毫秒
    on_phase(ConnectPhase::SshHandshake);
    let mut session = Session::new()?;
    session.set_timeout(SSH_PROTOCOL_TIMEOUT_MS);
    session.set_tcp_stream(tcp);
    session.handshake()?;

    // 根据认证类型执行对应的认证流程
    on_phase(ConnectPhase::Authenticating);
    match host.auth_type {
        AuthType::Password => {
            // 密码认证：运行时凭据必须由调用方从安全存储读取后传入
            let pwd =
                password.ok_or_else(|| AppError::InvalidHostConfig("密码为必填项".to_string()))?;
            session
                .userauth_password(&host.username, pwd)
                .map_err(|error| AppError::AuthenticationError(error.to_string()))?;
        }
        AuthType::PrivateKey => {
            // 私钥认证：私钥路径必须存在，口令为可选项
            let private_key = host
                .private_key_path
                .as_deref()
                .ok_or_else(|| AppError::InvalidHostConfig("私钥路径为必填项".to_string()))?;
            session
                .userauth_pubkey_file(&host.username, None, Path::new(private_key), passphrase)
                .map_err(|error| AppError::AuthenticationError(error.to_string()))?;
        }
    }

    // 双重校验：即使 userauth_* 未报错，也需确认认证状态
    if !session.authenticated() {
        return Err(AppError::AuthenticationError("SSH 认证失败".to_string()));
    }

    Ok(session)
}

/// 解析目标主机的所有可连接地址
///
/// 将主机名和端口解析为 `SocketAddr` 列表，供后续逐个尝试 TCP 建连。
fn resolve_socket_addrs(host: &HostConfig) -> Result<Vec<SocketAddr>, AppError> {
    let address = format!("{}:{}", host.host, host.port);
    let socket_addrs: Vec<SocketAddr> = address.to_socket_addrs()?.collect();
    if socket_addrs.is_empty() {
        return Err(AppError::SshConnectionError(format!(
            "连接失败: 未解析到可用地址 {address}"
        )));
    }
    Ok(socket_addrs)
}

/// 使用固定超时逐个尝试 TCP 建连
///
/// 只要任一地址连接成功即立即返回；若全部失败，则优先返回超时错误，
/// 否则返回最后一个非超时网络错误，确保上层可以准确映射状态。
fn connect_tcp_stream(
    socket_addrs: &[SocketAddr],
    timeout: Duration,
) -> Result<TcpStream, AppError> {
    let mut last_error: Option<std::io::Error> = None;
    let mut saw_timeout = false;

    for socket_addr in socket_addrs {
        match TcpStream::connect_timeout(socket_addr, timeout) {
            Ok(stream) => return Ok(stream),
            Err(error) => {
                if is_timeout_error(&error) {
                    saw_timeout = true;
                }
                last_error = Some(error);
            }
        }
    }

    if saw_timeout {
        return Err(build_connect_error(saw_timeout, last_error, timeout));
    }

    Err(build_connect_error(saw_timeout, last_error, timeout))
}

/// 判断底层 IO 错误是否属于连接超时语义
fn is_timeout_error(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
    )
}

/// 根据底层 TCP 建连结果组装统一的应用层错误
///
/// 若任一地址出现超时，则优先返回超时错误；否则回退到最后一个网络错误。
fn build_connect_error(
    saw_timeout: bool,
    last_error: Option<std::io::Error>,
    timeout: Duration,
) -> AppError {
    if saw_timeout {
        return AppError::SshConnectionError(format!(
            "Connection timeout after {}s",
            timeout.as_secs()
        ));
    }

    let error = last_error.unwrap_or_else(|| std::io::Error::other("unknown TCP connection error"));
    AppError::SshConnectionError(format!("连接失败: {error}"))
}

#[cfg(test)]
mod tests {
    use super::{
        ConnectPhase, SSH_PROTOCOL_TIMEOUT_MS, build_connect_error, connect_tcp_stream,
        is_timeout_error, resolve_socket_addrs,
    };
    use crate::errors::app_error::AppError;
    use crate::models::host::{AuthType, HostConfig};
    use std::io::ErrorKind;
    use std::net::SocketAddr;
    use std::time::Duration;

    /// 构造密码认证测试主机
    fn make_host(host: &str, port: u16) -> HostConfig {
        HostConfig {
            id: "host-1".to_string(),
            name: "test".to_string(),
            host: host.to_string(),
            port,
            username: "root".to_string(),
            auth_type: AuthType::Password,
            password_ref: Some("ref".to_string()),
            private_key_path: None,
            passphrase_ref: None,
            remark: None,
        }
    }

    /// 验证地址解析失败时返回 IO 层错误而非静默成功
    #[test]
    fn resolve_socket_addrs_returns_error_for_invalid_host() {
        let host = make_host("invalid host name with spaces", 22);
        let result = resolve_socket_addrs(&host);
        assert!(result.is_err(), "非法主机名应解析失败");
    }

    /// 验证 localhost 可被解析为至少一个可连接候选地址
    #[test]
    fn resolve_socket_addrs_returns_candidates_for_localhost() {
        let host = make_host("localhost", 22);
        let result = resolve_socket_addrs(&host).expect("localhost 应能解析成功");
        assert!(!result.is_empty(), "localhost 应至少解析出一个地址");
    }

    /// 验证存在超时时优先组装为 Connection timeout，避免 UI 长期停留在连接中
    #[test]
    fn build_connect_error_prefers_timeout_error() {
        let error = build_connect_error(
            true,
            Some(std::io::Error::new(ErrorKind::ConnectionRefused, "refused")),
            Duration::from_secs(10),
        );

        match error {
            AppError::SshConnectionError(message) => {
                assert!(
                    message.contains("Connection timeout"),
                    "存在超时时应优先返回超时错误，实际: {message}"
                );
            }
            other => panic!("期望超时错误，实际: {:?}", other),
        }
    }

    /// 验证纯非超时网络错误仍保持连接失败语义
    #[test]
    fn connect_tcp_stream_returns_connection_error_without_timeout() {
        let refused_addr: SocketAddr = "127.0.0.1:1".parse().expect("地址应合法");

        let result = connect_tcp_stream(&[refused_addr], Duration::from_millis(50));

        match result {
            Err(AppError::SshConnectionError(message)) => {
                assert!(
                    message.contains("连接失败"),
                    "非超时错误应保留连接失败语义: {message}"
                );
            }
            other => panic!("期望连接失败错误，实际: {:?}", other),
        }
    }

    /// 验证超时错误识别逻辑覆盖 TimedOut 与 WouldBlock
    #[test]
    fn is_timeout_error_recognizes_timeout_kinds() {
        let timed_out = std::io::Error::new(ErrorKind::TimedOut, "timed out");
        let would_block = std::io::Error::new(ErrorKind::WouldBlock, "would block");
        let refused = std::io::Error::new(ErrorKind::ConnectionRefused, "refused");

        assert!(is_timeout_error(&timed_out));
        assert!(is_timeout_error(&would_block));
        assert!(!is_timeout_error(&refused));
    }

    /// 验证 ConnectPhase 的序列化输出稳定，便于上层记录阶段日志
    #[test]
    fn connect_phase_serializes_to_stable_variant_name() {
        let value = serde_json::to_string(&ConnectPhase::Authenticating)
            .expect("阶段枚举序列化应成功");
        assert_eq!(value, "\"Authenticating\"");
    }

    /// 验证 SSH 协议阶段超时配置保持在 10 秒，便于定位卡死问题
    #[test]
    fn ssh_protocol_timeout_is_ten_seconds() {
        assert_eq!(SSH_PROTOCOL_TIMEOUT_MS, 10_000);
    }
}
