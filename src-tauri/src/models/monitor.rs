use serde::{Deserialize, Serialize};

/// 单张网卡接口的当前速率，由相邻累计字节计数计算。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NetworkInterface {
    /// 网卡接口名称。
    pub name: String,
    /// 接收速率（下行），未知时为 None。
    pub receive_bytes_per_second: Option<u64>,
    /// 发送速率（上行），未知时为 None。
    pub transmit_bytes_per_second: Option<u64>,
}

/// 网络采集结果，available 可区分采集失败和成功但没有候选接口。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NetworkSnapshot {
    /// /proc/net/dev 是否成功读取并解析。
    pub available: bool,
    /// 按远端返回顺序排列、且不含 lo 的候选网卡。
    pub interfaces: Vec<NetworkInterface>,
}

/// 服务器监控快照，由后端采集后推送给前端渲染
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MonitorSnapshot {
    pub session_id: String,
    /// 采集时间，Unix 毫秒时间戳
    pub timestamp: i64,
    /// CPU 使用率，0.0 ~ 100.0；无基线或采集缺失时为 null（未知）
    pub cpu_usage: Option<f64>,
    /// 内存使用率，0.0 ~ 100.0；MemTotal/MemAvailable 缺失时为 null（未知）
    pub memory_usage: Option<f64>,
    /// 内存总容量，单位字节；MemTotal 缺失/非法时为 null（未知）
    pub memory_total_bytes: Option<u64>,
    /// 内存已用量（MemTotal-MemAvailable），单位字节；任一字段缺失时为 null，
    /// MemAvailable 超出总量时按 0 已用处理，与使用率 clamp 语义一致
    pub memory_used_bytes: Option<u64>,
    /// 磁盘使用率，0.0 ~ 100.0；df 采集失败时为 null（未知）
    pub disk_usage: Option<f64>,
    /// 根分区剩余容量，单位字节；df 采集失败时为 null
    pub disk_available_bytes: Option<u64>,
    /// 根分区总容量，单位字节；df 采集失败时为 null
    pub disk_total_bytes: Option<u64>,
    /// 网络采集状态与全部候选网卡接口速率。
    pub network: NetworkSnapshot,
}

/// 长任务信息，所有持续任务必须可跟踪
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TaskInfo {
    pub task_id: String,
    pub task_type: String,
    /// 关联的会话 ID，部分任务可能不关联会话
    pub session_id: Option<String>,
    pub status: TaskStatus,
    /// 任务创建时间，Unix 毫秒时间戳
    pub created_at: i64,
}

/// 长任务状态枚举
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum TaskStatus {
    Pending,
    Running,
    Done,
    Failed,
}

#[cfg(test)]
#[path = "monitor_test.rs"]
mod tests;
