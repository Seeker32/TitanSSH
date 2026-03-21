use thiserror::Error;

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

    /// ssh2 库底层错误（握手失败、通道错误等），由 ssh2::Error 自动转换
    #[error("SSH 协议错误: {0}")]
    Ssh2Error(#[from] ssh2::Error),

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
}

/// 将 AppError 转换为 String，供 Tauri command 层返回给前端
impl From<AppError> for String {
    fn from(error: AppError) -> Self {
        error.to_string()
    }
}
