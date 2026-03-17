use serde::{Deserialize, Serialize};

/// 真实 SSH 会话信息，与前端 UI 标签页完全解耦
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
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
pub struct SessionStatusEvent {
    pub session_id: String,
    pub status: SessionStatus,
    /// 可选的错误详情文本
    pub message: Option<String>,
}

/// 终端数据流事件 Payload
#[derive(Debug, Clone, Serialize)]
pub struct TerminalDataEvent {
    pub session_id: String,
    pub data: String,
}

/// 长任务状态变更事件 Payload
#[derive(Debug, Clone, Serialize)]
pub struct TaskStatusEvent {
    pub task_id: String,
    pub status: crate::models::monitor::TaskStatus,
    /// 可选的错误详情文本
    pub message: Option<String>,
}
