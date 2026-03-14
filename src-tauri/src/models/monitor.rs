use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerStatus {
    pub session_id: String,
    pub ip: String,
    pub uptime_text: String,
    pub load1: f32,
    pub load5: f32,
    pub load15: f32,
    pub cpu_percent: f32,
    pub memory_used_mb: u64,
    pub memory_total_mb: u64,
    pub memory_percent: f32,
    pub swap_used_mb: u64,
    pub swap_total_mb: u64,
    pub swap_percent: f32,
    pub updated_at: i64,
}
