use serde::{Deserialize, Serialize};

/// 单个运行进程的结构化信息。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProcessInfo {
    pub pid: u32,
    pub ppid: u32,
    pub user: String,
    pub command: String,
    pub command_line: String,
    pub cpu_percent: Option<f64>,
    pub memory_bytes: Option<u64>,
    pub state: String,
}

/// 单次全量进程采样结果。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProcessSnapshot {
    pub session_id: String,
    pub timestamp: i64,
    pub processes: Vec<ProcessInfo>,
    pub total_count: usize,
}

#[cfg(test)]
#[path = "process_test.rs"]
mod tests;
