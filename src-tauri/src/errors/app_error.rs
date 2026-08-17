use serde::{Deserialize, Serialize};
use std::fmt;
use thiserror::Error;

/// 结构化错误详情，gettext msgid 风格：中文固定文案即翻译 key。
///
/// 固定文案与语言无关参数（底层错误文本、路径、endpoint 等）分离：
/// 中文模板留在后端日志里保持可读，前端按当前语言渲染翻译。
#[derive(Debug, Clone, PartialEq)]
pub enum ErrorDetail {
    /// 纯机器诊断文本（无固定文案），如底层库错误、文件路径；写入后端日志时保留原文，
    /// 跨 IPC 边界时由统一脱敏器处理。
    Raw(String),
    /// 中文固定文案模板 + 语言无关参数；模板用 {0}/{1} 占位参数
    Msg { key: String, params: Vec<String> },
}

impl ErrorDetail {
    /// 中文固定文案模板 + 语言无关参数（gettext msgid 风格 key）。
    pub fn msg(key: &str, params: Vec<String>) -> Self {
        Self::Msg {
            key: key.to_string(),
            params,
        }
    }
}

impl From<String> for ErrorDetail {
    fn from(text: String) -> Self {
        Self::Raw(text)
    }
}

impl fmt::Display for ErrorDetail {
    /// 后端日志渲染：模板占位按序替换；无占位的参数（with_appended_detail 追加）
    /// 以「；」连接在末尾。
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Raw(text) => write!(f, "{text}"),
            Self::Msg { key, params } => {
                let mut rendered = key.clone();
                for (index, param) in params.iter().enumerate() {
                    let placeholder = format!("{{{index}}}");
                    if rendered.contains(&placeholder) {
                        rendered = rendered.replacen(&placeholder, param, 1);
                    } else {
                        rendered.push('；');
                        rendered.push_str(param);
                    }
                }
                write!(f, "{rendered}")
            }
        }
    }
}

/// 跨 Tauri 边界的稳定错误 payload；code 为稳定英文代码供前端本地化摘要，
/// detail 为已脱敏的机器诊断（结构化详情时为 None），detailKey/detailParams 承载
/// 可翻译固定文案模板与已脱敏参数。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AppErrorInfo {
    pub code: String,
    /// 已统一脱敏的机器诊断文本（Raw 详情）；结构化详情时为 None。
    ///
    /// 不变量：detail 绝不包含凭据、口令、私钥内容。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// 固定文案翻译 key（gettext msgid，中文源文案）；前端按当前语言渲染
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail_key: Option<String>,
    /// 与 detailKey 模板 {0}/{1} 占位对应的语言无关参数；跨 IPC 时同样统一脱敏，
    /// 绝不包含凭据、口令、私钥内容。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail_params: Option<Vec<String>>,
}

/// 应用层错误枚举，覆盖 SSH 连接、认证、会话、存储等所有错误场景
///
/// 所有跨模块传递的错误均应转换为此枚举，
/// 避免在业务层直接暴露底层库的错误类型。
#[derive(Error, Debug)]
pub enum AppError {
    /// SSH TCP 连接失败（含超时、拒绝连接、网络不可达等）
    #[error("SSH 连接失败: {0}")]
    SshConnectionError(ErrorDetail),

    /// SSH 认证失败（密码错误、私钥不匹配、权限拒绝等）
    #[error("认证失败: {0}")]
    AuthenticationError(ErrorDetail),

    /// 指定 session_id 对应的会话不存在
    #[error("会话不存在: {0}")]
    SessionNotFound(ErrorDetail),

    /// 指定 host_id 对应的主机配置不存在（可能是删除主机后的过期前端引用）
    #[error("主机不存在: {0}")]
    HostNotFound(ErrorDetail),

    /// 主机配置不合法（必填字段缺失、格式错误等）
    #[error("主机配置无效: {0}")]
    InvalidHostConfig(ErrorDetail),

    /// 终端输入 IPC 请求缺少会话标识或未携带原始字节 payload
    #[error("终端输入无效: {0}")]
    InvalidTerminalInput(ErrorDetail),

    /// 持久化存储读写失败（JSON 序列化、文件 IO 等）
    #[error("存储错误: {0}")]
    StorageError(ErrorDetail),

    /// 底层 IO 错误，由标准库 std::io::Error 自动转换
    #[error("IO 错误: {0}")]
    IoError(#[from] std::io::Error),

    /// SSH 协议错误文本；第三方错误类型必须在 transport module 内转换
    #[error("SSH 协议错误: {0}")]
    SshProtocolError(ErrorDetail),

    /// OS 安全存储访问失败（Keychain / Credential Manager / Secret Service）
    #[error("安全存储错误: {0}")]
    SecureStoreError(ErrorDetail),

    /// 凭据在安全存储中不存在（主机配置可能已损坏或凭据未写入）
    #[error("凭据不存在: {0}，请重新编辑主机配置以重新保存凭据")]
    CredentialNotFound(ErrorDetail),

    /// SFTP 子通道建立失败（含 SSH session 已断开）
    #[error("SFTP 通道错误: {0}")]
    SftpChannelError(ErrorDetail),

    /// 无权限访问远程路径
    #[error("SFTP 权限拒绝: {0}")]
    SftpPermissionDenied(ErrorDetail),

    /// 远程路径不存在
    #[error("SFTP 路径不存在: {0}")]
    SftpPathNotFound(ErrorDetail),

    /// 传输中断（含传输中通道断开）
    #[error("SFTP 传输错误: {0}")]
    SftpTransferError(ErrorDetail),

    /// 本地文件打开失败（上传读取源）
    #[error("SFTP 打开失败: {0}")]
    SftpOpenError(ErrorDetail),

    /// 传输读取失败（远端读取或本地读取）
    #[error("SFTP 读取失败: {0}")]
    SftpReadError(ErrorDetail),

    /// 传输写入失败（远端写入或本地写入）
    #[error("SFTP 写入失败: {0}")]
    SftpWriteError(ErrorDetail),

    /// 目标文件创建失败（本地或远端）
    #[error("SFTP 创建失败: {0}")]
    SftpCreateError(ErrorDetail),

    /// 取消目标任务不存在（未入队或已从 registry 移除）
    #[error("SFTP 任务不存在: {0}")]
    SftpTaskNotFound(ErrorDetail),

    /// 监控任务不存在（从未创建、已停止或已过期）；stop_monitoring 对
    /// 注册表 miss 时上报，前端据此区分「已停止」与「早已消失」
    #[error("监控任务不存在: {0}")]
    MonitorTaskNotFound(ErrorDetail),

    /// 会话存在但尚无监控快照（首轮采集完成前、或监控已停止/失败）；
    /// 与 SessionNotFound 严格区分：SessionNotFound 是 close_session 式
    /// teardown 的键，瞬时无数据不得触发前端拆除会话状态
    #[error("监控快照尚不可用: {0}")]
    MonitorSnapshotUnavailable(ErrorDetail),

    /// 监控采集输出不含任何指标键（脚本未执行、awk/df 缺失、shell 受限等）；
    /// 与个别字段缺失的未知语义不同，零指标键说明采集管线整体损坏，
    /// 必须终止任务而非每 2 秒发布一个全 None 的退化快照
    #[error("监控采集输出无效: {0}")]
    MonitorCollectionError(ErrorDetail),

    /// 下载目标已存在且冲突策略为 Reject（前端据此逐文件确认覆盖）
    #[error("SFTP 目标已存在: {0}")]
    SftpTargetExists(ErrorDetail),

    /// 同一 Session 已有 Pending/Running 下载占用相同最终目标
    #[error("SFTP 目标正被占用: {0}")]
    SftpTargetBusy(ErrorDetail),

    /// 临时文件发布到最终目标失败（原目标文件不受影响）
    #[error("SFTP 发布失败: {0}")]
    SftpPublishError(ErrorDetail),

    /// 用户拒绝了未知主机身份；detail 携带 endpoint 与指纹，连接不得进入认证
    #[error("已拒绝未知主机身份: {0}")]
    HostKeyRejected(ErrorDetail),

    /// 主机身份确认请求不存在（已解决或从未创建）
    #[error("主机身份确认请求不存在: {0}")]
    HostKeyChallengeNotFound(ErrorDetail),

    /// 等待主机身份确认期间会话被关闭，验证已取消
    #[error("主机身份验证已取消: {0}")]
    HostKeyVerificationCancelled(ErrorDetail),

    /// TitanSSH 独立信任存储不可读、不可解析或写入失败；fail-closed，绝不静默视为空
    #[error("信任存储错误: {0}")]
    TrustStoreError(ErrorDetail),

    /// "接受并保存"持久化失败；challenge 保持未决，不自动降级为临时信任
    #[error("主机信任保存失败: {0}")]
    HostKeySaveFailed(ErrorDetail),

    /// HostConfig 保存/删除后的 endpoint 信任记录自动清理失败；
    /// 配置变更已生效，但清理未完成的管理动作必须显式报错，不得静默报告为成功
    #[error("主机信任记录清理失败: {0}")]
    HostTrustCleanupFailed(ErrorDetail),

    /// 日志导出保存对话框选中的文件无法解析为本地路径（云端/虚拟文件系统 URL
    /// 未落地）；专用 code 供前端按语言本地化摘要，detail 只携带底层诊断
    #[error("无法解析保存路径: {0}")]
    LogExportPathResolveFailed(ErrorDetail),

    /// 前端传入的日志等级不在支持列表内；携带输入值供诊断，不得伪装成主机配置错误
    #[error("无效的日志等级: {0}")]
    InvalidLogLevel(ErrorDetail),
}

impl AppError {
    /// 返回用于日志与 IPC 的稳定英文错误代码。
    pub fn code(&self) -> &'static str {
        match self {
            Self::SshConnectionError(_) => "SshConnectionError",
            Self::AuthenticationError(_) => "AuthenticationError",
            Self::SessionNotFound(_) => "SessionNotFound",
            Self::HostNotFound(_) => "HostNotFound",
            Self::InvalidHostConfig(_) => "InvalidHostConfig",
            Self::InvalidTerminalInput(_) => "InvalidTerminalInput",
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
            Self::MonitorTaskNotFound(_) => "MonitorTaskNotFound",
            Self::MonitorSnapshotUnavailable(_) => "MonitorSnapshotUnavailable",
            Self::MonitorCollectionError(_) => "MonitorCollectionError",
            Self::SftpTargetExists(_) => "SftpTargetExists",
            Self::SftpTargetBusy(_) => "SftpTargetBusy",
            Self::SftpPublishError(_) => "SftpPublishError",
            Self::HostKeyRejected(_) => "HostKeyRejected",
            Self::HostKeyChallengeNotFound(_) => "HostKeyChallengeNotFound",
            Self::HostKeyVerificationCancelled(_) => "HostKeyVerificationCancelled",
            Self::TrustStoreError(_) => "TrustStoreError",
            Self::HostKeySaveFailed(_) => "HostKeySaveFailed",
            Self::HostTrustCleanupFailed(_) => "HostTrustCleanupFailed",
            Self::LogExportPathResolveFailed(_) => "LogExportPathResolveFailed",
            Self::InvalidLogLevel(_) => "InvalidLogLevel",
        }
    }

    /// 保持错误代码不变，把补充说明追加到详情末尾。
    ///
    /// 用于复合诊断（如传输失败叠加临时文件清理失败）：
    /// 主错误代码仍是前端判定的稳定依据，详情拼上清理失败的具体信息。
    /// Raw 详情追加文本；Msg 详情把补充说明作为额外参数（模板占位之外的参数
    /// 由 Display/前端渲染时以「；」连接在末尾）。
    pub fn with_appended_detail(self, extra: &str) -> AppError {
        fn append(payload: ErrorDetail, extra: &str) -> ErrorDetail {
            match payload {
                ErrorDetail::Raw(text) => ErrorDetail::Raw(format!("{text}；{extra}")),
                ErrorDetail::Msg { key, mut params } => {
                    params.push(extra.to_string());
                    ErrorDetail::Msg { key, params }
                }
            }
        }
        match self {
            Self::SshConnectionError(p) => Self::SshConnectionError(append(p, extra)),
            Self::AuthenticationError(p) => Self::AuthenticationError(append(p, extra)),
            Self::SessionNotFound(p) => Self::SessionNotFound(append(p, extra)),
            Self::HostNotFound(p) => Self::HostNotFound(append(p, extra)),
            Self::InvalidHostConfig(p) => Self::InvalidHostConfig(append(p, extra)),
            Self::InvalidTerminalInput(p) => Self::InvalidTerminalInput(append(p, extra)),
            Self::StorageError(p) => Self::StorageError(append(p, extra)),
            Self::IoError(io) => Self::IoError(std::io::Error::other(format!("{io}；{extra}"))),
            Self::SshProtocolError(p) => Self::SshProtocolError(append(p, extra)),
            Self::SecureStoreError(p) => Self::SecureStoreError(append(p, extra)),
            Self::CredentialNotFound(p) => Self::CredentialNotFound(append(p, extra)),
            Self::SftpChannelError(p) => Self::SftpChannelError(append(p, extra)),
            Self::SftpPermissionDenied(p) => Self::SftpPermissionDenied(append(p, extra)),
            Self::SftpPathNotFound(p) => Self::SftpPathNotFound(append(p, extra)),
            Self::SftpTransferError(p) => Self::SftpTransferError(append(p, extra)),
            Self::SftpOpenError(p) => Self::SftpOpenError(append(p, extra)),
            Self::SftpReadError(p) => Self::SftpReadError(append(p, extra)),
            Self::SftpWriteError(p) => Self::SftpWriteError(append(p, extra)),
            Self::SftpCreateError(p) => Self::SftpCreateError(append(p, extra)),
            Self::SftpTaskNotFound(p) => Self::SftpTaskNotFound(append(p, extra)),
            Self::MonitorTaskNotFound(p) => Self::MonitorTaskNotFound(append(p, extra)),
            Self::MonitorSnapshotUnavailable(p) => {
                Self::MonitorSnapshotUnavailable(append(p, extra))
            }
            Self::MonitorCollectionError(p) => Self::MonitorCollectionError(append(p, extra)),
            Self::SftpTargetExists(p) => Self::SftpTargetExists(append(p, extra)),
            Self::SftpTargetBusy(p) => Self::SftpTargetBusy(append(p, extra)),
            Self::SftpPublishError(p) => Self::SftpPublishError(append(p, extra)),
            Self::HostKeyRejected(p) => Self::HostKeyRejected(append(p, extra)),
            Self::HostKeyChallengeNotFound(p) => Self::HostKeyChallengeNotFound(append(p, extra)),
            Self::HostKeyVerificationCancelled(p) => {
                Self::HostKeyVerificationCancelled(append(p, extra))
            }
            Self::TrustStoreError(p) => Self::TrustStoreError(append(p, extra)),
            Self::HostKeySaveFailed(p) => Self::HostKeySaveFailed(append(p, extra)),
            Self::HostTrustCleanupFailed(p) => Self::HostTrustCleanupFailed(append(p, extra)),
            Self::LogExportPathResolveFailed(p) => {
                Self::LogExportPathResolveFailed(append(p, extra))
            }
            Self::InvalidLogLevel(p) => Self::InvalidLogLevel(append(p, extra)),
        }
    }
}

/// 脱敏 PEM/OpenSSH 私钥块，防止完整私钥内容跨越 IPC 边界。
fn redact_private_key_blocks(text: &str) -> String {
    const BEGIN_PREFIX: &str = "-----BEGIN ";

    let mut remaining = text;
    let mut redacted = String::with_capacity(text.len());
    while let Some(begin_offset) = remaining.find(BEGIN_PREFIX) {
        let header_start = begin_offset + BEGIN_PREFIX.len();
        let Some(header_suffix_offset) = remaining[header_start..].find("-----") else {
            if remaining[begin_offset..].contains("PRIVATE KEY") {
                redacted.push_str(&remaining[..begin_offset]);
                redacted.push_str("[REDACTED]");
                return redacted;
            }
            redacted.push_str(remaining);
            return redacted;
        };
        let header_end = header_start + header_suffix_offset + "-----".len();
        let header = &remaining[begin_offset..header_end];
        if !header.contains("PRIVATE KEY") {
            redacted.push_str(&remaining[..header_end]);
            remaining = &remaining[header_end..];
            continue;
        }

        let end_marker = header.replacen("BEGIN", "END", 1);
        redacted.push_str(&remaining[..begin_offset]);
        redacted.push_str("[REDACTED]");
        let body = &remaining[header_end..];
        match body.find(&end_marker) {
            Some(end_offset) => remaining = &body[end_offset + end_marker.len()..],
            None => return redacted,
        }
    }
    redacted.push_str(remaining);
    redacted
}

/// 脱敏带有敏感字段名的值，覆盖底层库常见的 `password=...`、`passphrase: ...` 等诊断格式。
fn redact_labeled_sensitive_values(text: &str) -> String {
    const LABELS: [&str; 7] = [
        "password",
        "passphrase",
        "credential",
        "secret",
        "private_key",
        "privatekey",
        "private-key",
    ];

    let mut remaining = text;
    let mut redacted = String::with_capacity(text.len());
    loop {
        let normalized = remaining.to_ascii_lowercase();
        let candidate = LABELS
            .iter()
            .filter_map(|label| normalized.find(label).map(|offset| (offset, *label)))
            .min_by_key(|(offset, _)| *offset);
        let Some((label_offset, label)) = candidate else {
            redacted.push_str(remaining);
            return redacted;
        };

        let label_end = label_offset + label.len();
        let mut separator_offset = label_end;
        while let Some(character) = remaining[separator_offset..].chars().next() {
            if !character.is_whitespace() {
                break;
            }
            separator_offset += character.len_utf8();
        }
        let separator_len = match remaining[separator_offset..].chars().next() {
            Some(character @ ('=' | ':' | '：')) => character.len_utf8(),
            _ => {
                redacted.push_str(&remaining[..label_end]);
                remaining = &remaining[label_end..];
                continue;
            }
        };
        let mut value_start = separator_offset + separator_len;
        while let Some(character) = remaining[value_start..].chars().next() {
            if !character.is_whitespace() {
                break;
            }
            value_start += character.len_utf8();
        }
        let value_end = remaining[value_start..]
            .char_indices()
            .find_map(|(offset, character)| {
                matches!(character, ';' | '；' | ',' | '\n' | '\r').then_some(value_start + offset)
            })
            .unwrap_or(remaining.len());
        redacted.push_str(&remaining[..value_start]);
        redacted.push_str("[REDACTED]");
        remaining = &remaining[value_end..];
    }
}

/// 统一清理即将进入 renderer 的自由诊断文本；日志路径不调用此函数以保留排障上下文。
fn redact_ipc_diagnostic(text: &str) -> String {
    redact_labeled_sensitive_values(&redact_private_key_blocks(text))
}

/// 将内部错误转换为语言无关的 IPC 错误：Raw 详情走 detail 字段（纯机器文本），
/// Msg 详情拆成 detailKey（中文模板，前端按语言翻译）+ detailParams。
///
/// 这是 AppError 到 renderer 的统一自由文本出口：所有动态详情统一脱敏，确保凭据、
/// 口令和私钥内容不会通过 AppError 的 Tauri IPC payload 暴露给前端。
impl From<&AppError> for AppErrorInfo {
    fn from(error: &AppError) -> Self {
        let code = error.code();
        let payload = match error {
            AppError::SshConnectionError(p)
            | AppError::AuthenticationError(p)
            | AppError::SessionNotFound(p)
            | AppError::HostNotFound(p)
            | AppError::InvalidHostConfig(p)
            | AppError::InvalidTerminalInput(p)
            | AppError::StorageError(p)
            | AppError::SshProtocolError(p)
            | AppError::SecureStoreError(p)
            | AppError::CredentialNotFound(p)
            | AppError::SftpChannelError(p)
            | AppError::SftpPermissionDenied(p)
            | AppError::SftpPathNotFound(p)
            | AppError::SftpTransferError(p)
            | AppError::SftpOpenError(p)
            | AppError::SftpReadError(p)
            | AppError::SftpWriteError(p)
            | AppError::SftpCreateError(p)
            | AppError::SftpTaskNotFound(p)
            | AppError::MonitorTaskNotFound(p)
            | AppError::MonitorSnapshotUnavailable(p)
            | AppError::MonitorCollectionError(p)
            | AppError::SftpTargetExists(p)
            | AppError::SftpTargetBusy(p)
            | AppError::SftpPublishError(p)
            | AppError::HostKeyRejected(p)
            | AppError::HostKeyChallengeNotFound(p)
            | AppError::HostKeyVerificationCancelled(p)
            | AppError::TrustStoreError(p)
            | AppError::HostKeySaveFailed(p)
            | AppError::HostTrustCleanupFailed(p)
            | AppError::LogExportPathResolveFailed(p)
            | AppError::InvalidLogLevel(p) => Some(p.clone()),
            AppError::IoError(io) => Some(ErrorDetail::Raw(io.to_string())),
        };
        let (detail, detail_key, detail_params) = match payload {
            Some(ErrorDetail::Raw(text)) => (Some(redact_ipc_diagnostic(&text)), None, None),
            Some(ErrorDetail::Msg { key, params }) => (
                None,
                Some(key),
                Some(
                    params
                        .into_iter()
                        .map(|param| redact_ipc_diagnostic(&param))
                        .collect(),
                ),
            ),
            None => (None, None, None),
        };
        Self {
            code: code.to_string(),
            detail,
            detail_key,
            detail_params,
        }
    }
}

impl From<AppError> for AppErrorInfo {
    fn from(error: AppError) -> Self {
        Self::from(&error)
    }
}

#[cfg(test)]
#[path = "app_error_test.rs"]
mod tests;
