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

    /// 下载目标已存在且冲突策略为 Reject（前端据此逐文件确认覆盖）
    #[error("SFTP 目标已存在: {0}")]
    SftpTargetExists(String),

    /// 同一 Session 已有 Pending/Running 下载占用相同最终目标
    #[error("SFTP 目标正被占用: {0}")]
    SftpTargetBusy(String),

    /// 临时文件发布到最终目标失败（原目标文件不受影响）
    #[error("SFTP 发布失败: {0}")]
    SftpPublishError(String),

    /// 用户拒绝了未知主机身份；detail 携带 endpoint 与指纹，连接不得进入认证
    #[error("已拒绝未知主机身份: {0}")]
    HostKeyRejected(String),

    /// 主机身份确认请求不存在（已解决或从未创建）
    #[error("主机身份确认请求不存在: {0}")]
    HostKeyChallengeNotFound(String),

    /// 等待主机身份确认期间会话被关闭，验证已取消
    #[error("主机身份验证已取消: {0}")]
    HostKeyVerificationCancelled(String),

    /// TitanSSH 独立信任存储不可读、不可解析或写入失败；fail-closed，绝不静默视为空
    #[error("信任存储错误: {0}")]
    TrustStoreError(String),

    /// "接受并保存"持久化失败；challenge 保持未决，不自动降级为临时信任
    #[error("主机信任保存失败: {0}")]
    HostKeySaveFailed(String),

    /// HostConfig 保存/删除后的 endpoint 信任记录自动清理失败；
    /// 配置变更已生效，但清理未完成的管理动作必须显式报错，不得静默报告为成功
    #[error("主机信任记录清理失败: {0}")]
    HostTrustCleanupFailed(String),
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
            Self::SftpTargetExists(_) => "SftpTargetExists",
            Self::SftpTargetBusy(_) => "SftpTargetBusy",
            Self::SftpPublishError(_) => "SftpPublishError",
            Self::HostKeyRejected(_) => "HostKeyRejected",
            Self::HostKeyChallengeNotFound(_) => "HostKeyChallengeNotFound",
            Self::HostKeyVerificationCancelled(_) => "HostKeyVerificationCancelled",
            Self::TrustStoreError(_) => "TrustStoreError",
            Self::HostKeySaveFailed(_) => "HostKeySaveFailed",
            Self::HostTrustCleanupFailed(_) => "HostTrustCleanupFailed",
        }
    }

    /// 保持错误代码不变，把补充说明追加到 detail 末尾。
    ///
    /// 用于复合诊断（如传输失败叠加临时文件清理失败）：
    /// 主错误代码仍是前端判定的稳定依据，detail 拼上清理失败的具体信息。
    pub fn with_appended_detail(self, extra: &str) -> AppError {
        let merged = format!("{}；{}", self, extra);
        match self {
            Self::SshConnectionError(_) => Self::SshConnectionError(merged),
            Self::AuthenticationError(_) => Self::AuthenticationError(merged),
            Self::SessionNotFound(_) => Self::SessionNotFound(merged),
            Self::InvalidHostConfig(_) => Self::InvalidHostConfig(merged),
            Self::StorageError(_) => Self::StorageError(merged),
            Self::IoError(_) => Self::IoError(std::io::Error::other(merged)),
            Self::SshProtocolError(_) => Self::SshProtocolError(merged),
            Self::SecureStoreError(_) => Self::SecureStoreError(merged),
            Self::CredentialNotFound(_) => Self::CredentialNotFound(merged),
            Self::SftpChannelError(_) => Self::SftpChannelError(merged),
            Self::SftpPermissionDenied(_) => Self::SftpPermissionDenied(merged),
            Self::SftpPathNotFound(_) => Self::SftpPathNotFound(merged),
            Self::SftpTransferError(_) => Self::SftpTransferError(merged),
            Self::SftpOpenError(_) => Self::SftpOpenError(merged),
            Self::SftpReadError(_) => Self::SftpReadError(merged),
            Self::SftpWriteError(_) => Self::SftpWriteError(merged),
            Self::SftpCreateError(_) => Self::SftpCreateError(merged),
            Self::SftpTaskNotFound(_) => Self::SftpTaskNotFound(merged),
            Self::SftpTargetExists(_) => Self::SftpTargetExists(merged),
            Self::SftpTargetBusy(_) => Self::SftpTargetBusy(merged),
            Self::SftpPublishError(_) => Self::SftpPublishError(merged),
            Self::HostKeyRejected(_) => Self::HostKeyRejected(merged),
            Self::HostKeyChallengeNotFound(_) => Self::HostKeyChallengeNotFound(merged),
            Self::HostKeyVerificationCancelled(_) => Self::HostKeyVerificationCancelled(merged),
            Self::TrustStoreError(_) => Self::TrustStoreError(merged),
            Self::HostKeySaveFailed(_) => Self::HostKeySaveFailed(merged),
            Self::HostTrustCleanupFailed(_) => Self::HostTrustCleanupFailed(merged),
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
            | AppError::SftpTaskNotFound(detail)
            | AppError::SftpTargetExists(detail)
            | AppError::SftpTargetBusy(detail)
            | AppError::SftpPublishError(detail) => detail,
            AppError::HostKeyRejected(detail)
            | AppError::HostKeyChallengeNotFound(detail)
            | AppError::HostKeyVerificationCancelled(detail)
            | AppError::TrustStoreError(detail)
            | AppError::HostKeySaveFailed(detail)
            | AppError::HostTrustCleanupFailed(detail) => detail,
            AppError::IoError(detail) => detail.to_string(),
        };
        Self {
            code: code.to_string(),
            detail: Some(detail),
        }
    }
}

#[cfg(test)]
#[path = "app_error_test.rs"]
mod tests;
