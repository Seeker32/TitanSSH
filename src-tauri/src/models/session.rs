use crate::errors::app_error::AppErrorInfo;
use serde::{Deserialize, Serialize};

/// 真实 SSH 会话信息，与前端 UI 标签页完全解耦
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    pub session_id: String,
    pub host_id: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub status: SessionStatus,
    /// 会话创建时间，Unix 毫秒时间戳
    pub created_at: i64,
}

/// 会话状态枚举
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum SessionStatus {
    Connecting,
    Connected,
    AuthFailed,
    Disconnected,
    Timeout,
    Error,
}

/// 会话状态变更事件 Payload
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionStatusEvent {
    pub session_id: String,
    pub status: SessionStatus,
    /// 可选的语言无关错误。
    pub error: Option<AppErrorInfo>,
}

/// 终端数据流事件 Payload
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalDataEvent {
    pub session_id: String,
    /// 终端的 UTF-8 文本片段。生产端跨底层读取边界增量解码，绝不拆分完整 UTF-8
    /// 字符；非 UTF-8 字节及 EOF 时未完成的序列会以 U+FFFD 替换。
    ///
    /// 此字段不是字节保真通道，不能用于通过 PTY 传输二进制数据；需要二进制传输时
    /// 必须使用 SFTP 服务或定义带编码的字节载荷协议。
    pub data: String,
}

/// 长任务状态变更事件 Payload
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskStatusEvent {
    pub task_id: String,
    /// 采样任务类型，用于前端按结构化归属路由事件。
    pub task_type: String,
    /// 采样任务所属 Runtime Session。
    pub session_id: String,
    pub status: crate::models::monitor::TaskStatus,
    /// 可选的语言无关错误。
    pub error: Option<AppErrorInfo>,
}

/// 主机身份确认事件 Payload（host-identity:challenge）
/// 指纹由 Rust 侧计算，前端不解析 SSH key 文本。
/// Unknown：信任存储无该 endpoint 记录；Changed：已保存记录与呈现 key 不一致。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HostIdentityChallenge {
    pub challenge_id: String,
    pub session_id: String,
    pub host: String,
    pub port: u16,
    /// challenge 类型：未知主机或已保存 key 与呈现不一致
    pub kind: HostIdentityChallengeKind,
    /// OpenSSH 风格算法名（如 ssh-ed25519）；Changed 时为服务器本次呈现的算法
    pub key_algorithm: String,
    /// OpenSSH 风格 SHA-256 指纹；Changed 时为服务器本次呈现 key 的指纹
    pub fingerprint: String,
    /// Changed challenge 专属：已保存信任记录的算法名；Unknown 为 None
    pub stored_algorithm: Option<String>,
    /// Changed challenge 专属：已保存信任记录的 SHA-256 指纹（后端由 blob 计算）；Unknown 为 None
    pub stored_fingerprint: Option<String>,
    /// 事件产生时间，Unix 毫秒时间戳
    pub timestamp: i64,
}

/// 主机身份确认撤销事件 Payload（host-identity:challenge-dismissed）
/// 后端移除未决 challenge（被新指纹取代、会话关闭、异地解决或应用退出）时
/// 通知前端撤下对应确认卡，UI 不得残留孤儿提示。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HostIdentityChallengeDismissed {
    pub challenge_id: String,
    pub session_id: String,
}

/// challenge 类型：未知主机 / 已保存记录与呈现 key 不一致（主机指纹变化）
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum HostIdentityChallengeKind {
    /// 信任存储没有该 endpoint 记录（首次连接）
    Unknown,
    /// 已保存记录与呈现 key 的算法或公钥材料不一致
    Changed,
}
