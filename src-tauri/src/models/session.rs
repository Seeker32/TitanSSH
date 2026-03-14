use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionInfo {
    pub session_id: String,
    pub host_id: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub status: SessionStatus,
    pub created_at: i64,
    pub active: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum SessionStatus {
    Connecting,
    Connected,
    Disconnected,
    Failed,
}
