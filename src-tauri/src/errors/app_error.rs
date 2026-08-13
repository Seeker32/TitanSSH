use serde::{Deserialize, Serialize};
use thiserror::Error;

/// 跨 Tauri 边界的稳定错误 payload；detail 保留底层诊断供前端本地化摘要后展示。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AppErrorInfo {
    pub code: String,
    pub detail: Option<String>,
}

/// 应用层错误枚举，覆盖 SSH 连接、认证、会话、存储等所有错误场景
///
/// 所有跨模块传递的错误均应转换为此枚举，
/// 避免在业务层直接暴露底层库的错误类型。
#[derive(Error, Debug)]
pub enum AppError {
    /// SSH TCP 连接失败（含超时、拒绝连接、网络不可达等）
    #[error("SSH 连接失败: {0}")]
    SshConnectionError(String),

    /// SSH 认证失败（密码错误、私钥不匹配、权限拒绝等）
    #[error("认证失败: {0}")]
    AuthenticationError(String),

    /// 指定 session_id 对应的会话不存在
    #[error("会话不存在: {0}")]
    SessionNotFound(String),

    /// 主机配置不合法（必填字段缺失、格式错误等）
    #[error("主机配置无效: {0}")]
    InvalidHostConfig(String),

    /// 持久化存储读写失败（JSON 序列化、文件 IO 等）
    #[error("存储错误: {0}")]
    StorageError(String),

    /// 底层 IO 错误，由标准库 std::io::Error 自动转换
    #[error("IO 错误: {0}")]
    IoError(#[from] std::io::Error),

    /// SSH 协议错误文本；第三方错误类型必须在 transport module 内转换
    #[error("SSH 协议错误: {0}")]
    SshProtocolError(String),

    /// OS 安全存储访问失败（Keychain / Credential Manager / Secret Service）
    #[error("安全存储错误: {0}")]
    SecureStoreError(String),

    /// 凭据在安全存储中不存在（主机配置可能已损坏或凭据未写入）
    #[error("凭据不存在: {0}，请重新编辑主机配置以重新保存凭据")]
    CredentialNotFound(String),

    /// SFTP 子通道建立失败（含 SSH session 已断开）
    #[error("SFTP 通道错误: {0}")]
    SftpChannelError(String),

    /// 无权限访问远程路径
    #[error("SFTP 权限拒绝: {0}")]
    SftpPermissionDenied(String),

    /// 远程路径不存在
    #[error("SFTP 路径不存在: {0}")]
    SftpPathNotFound(String),

    /// 传输中断（含传输中通道断开）
    #[error("SFTP 传输错误: {0}")]
    SftpTransferError(String),

    /// 本地文件打开失败（上传读取源）
    #[error("SFTP 打开失败: {0}")]
    SftpOpenError(String),

    /// 传输读取失败（远端读取或本地读取）
    #[error("SFTP 读取失败: {0}")]
    SftpReadError(String),

    /// 传输写入失败（远端写入或本地写入）
    #[error("SFTP 写入失败: {0}")]
    SftpWriteError(String),

    /// 目标文件创建失败（本地或远端）
    #[error("SFTP 创建失败: {0}")]
    SftpCreateError(String),

    /// 取消目标任务不存在（未入队或已从 registry 移除）
    #[error("SFTP 任务不存在: {0}")]
    SftpTaskNotFound(String),
}

impl AppError {
    /// 返回用于日志与 IPC 的稳定英文错误代码。
    pub fn code(&self) -> &'static str {
        match self {
            Self::SshConnectionError(_) => "SshConnectionError",
            Self::AuthenticationError(_) => "AuthenticationError",
            Self::SessionNotFound(_) => "SessionNotFound",
            Self::InvalidHostConfig(_) => "InvalidHostConfig",
            Self::StorageError(_) => "StorageError",
            Self::IoError(_) => "IoError",
            Self::SshProtocolError(_) => "SshProtocolError",
            Self::SecureStoreError(_) => "SecureStoreError",
            Self::CredentialNotFound(_) => "CredentialNotFound",
            Self::SftpChannelError(_) => "SftpChannelError",
            Self::SftpPermissionDenied(_) => "SftpPermissionDenied",
            Self::SftpPathNotFound(_) => "SftpPathNotFound",
            Self::SftpTransferError(_) => "SftpTransferError",
            Self::SftpOpenError(_) => "SftpOpenError",
            Self::SftpReadError(_) => "SftpReadError",
            Self::SftpWriteError(_) => "SftpWriteError",
            Self::SftpCreateError(_) => "SftpCreateError",
            Self::SftpTaskNotFound(_) => "SftpTaskNotFound",
        }
    }
}

/// 将内部错误转换为语言无关的 IPC 错误。
impl From<AppError> for AppErrorInfo {
    fn from(error: AppError) -> Self {
        let code = error.code();
        let detail = match error {
            AppError::SshConnectionError(detail)
            | AppError::AuthenticationError(detail)
            | AppError::SessionNotFound(detail)
            | AppError::InvalidHostConfig(detail)
            | AppError::StorageError(detail)
            | AppError::SshProtocolError(detail)
            | AppError::SecureStoreError(detail)
            | AppError::CredentialNotFound(detail)
            | AppError::SftpChannelError(detail)
            | AppError::SftpPermissionDenied(detail)
            | AppError::SftpPathNotFound(detail)
            | AppError::SftpTransferError(detail)
            | AppError::SftpOpenError(detail)
            | AppError::SftpReadError(detail)
            | AppError::SftpWriteError(detail)
            | AppError::SftpCreateError(detail)
            | AppError::SftpTaskNotFound(detail) => detail,
            AppError::IoError(detail) => detail.to_string(),
        };
        Self {
            code: code.to_string(),
            detail: Some(detail),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{AppError, AppErrorInfo};

    /// SSH 协议错误只保存稳定文本，不向所属 module 外泄漏 ssh2 错误类型。
    #[test]
    fn ssh_protocol_error_contains_only_stable_text() {
        let error = AppError::SshProtocolError("channel failed".to_string());

        assert_eq!(error.to_string(), "SSH 协议错误: channel failed");
    }

    /// 日志错误代码始终使用稳定英文标识。
    #[test]
    fn app_error_code_is_english_and_stable() {
        assert_eq!(
            AppError::CredentialNotFound("主机密码".to_string()).code(),
            "CredentialNotFound"
        );
    }

    /// IPC 错误使用稳定代码与 camelCase detail，不携带已本地化 UI 文案。
    #[test]
    fn app_error_info_serializes_as_structured_payload() {
        let value = serde_json::to_value(AppErrorInfo::from(AppError::AuthenticationError(
            "denied".to_string(),
        )))
        .expect("错误 payload 应序列化");
        assert_eq!(
            value,
            serde_json::json!({ "code": "AuthenticationError", "detail": "denied" })
        );
    }
}
