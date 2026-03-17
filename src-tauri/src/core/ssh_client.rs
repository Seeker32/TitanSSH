use crate::errors::app_error::AppError;
use crate::models::host::{AuthType, HostConfig};
use ssh2::Session;
use std::net::TcpStream;
use std::path::Path;
use std::time::Duration;

/// 建立 SSH 连接并完成认证，返回已认证的 Session
/// - host: 主机配置（不含明文凭据）
/// - password: 运行时从安全存储读取的明文密码（Password 认证时必须提供）
/// - passphrase: 运行时从安全存储读取的明文私钥口令（PrivateKey 认证时可选）
pub fn connect(
    host: &HostConfig,
    password: Option<&str>,
    passphrase: Option<&str>,
) -> Result<Session, AppError> {
    let tcp = TcpStream::connect(format!("{}:{}", host.host, host.port)).map_err(|error| {
        if matches!(
            error.kind(),
            std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
        ) {
            AppError::SshConnectionError(format!("Connection timeout: {error}"))
        } else {
            AppError::SshConnectionError(format!("Failed to connect: {error}"))
        }
    })?;

    tcp.set_read_timeout(Some(Duration::from_millis(250)))?;
    tcp.set_write_timeout(Some(Duration::from_secs(3)))?;

    let mut session = Session::new()?;
    session.set_tcp_stream(tcp);
    session.handshake()?;

    match host.auth_type {
        AuthType::Password => {
            let pwd = password.ok_or_else(|| {
                AppError::InvalidHostConfig("Password is required".to_string())
            })?;
            session
                .userauth_password(&host.username, pwd)
                .map_err(|error| AppError::AuthenticationError(error.to_string()))?;
        }
        AuthType::PrivateKey => {
            let private_key = host.private_key_path.as_deref().ok_or_else(|| {
                AppError::InvalidHostConfig("Private key path is required".to_string())
            })?;
            session
                .userauth_pubkey_file(
                    &host.username,
                    None,
                    Path::new(private_key),
                    passphrase,
                )
                .map_err(|error| AppError::AuthenticationError(error.to_string()))?;
        }
    }

    if !session.authenticated() {
        return Err(AppError::AuthenticationError(
            "SSH authentication failed".to_string(),
        ));
    }

    Ok(session)
}
