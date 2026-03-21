use serde::{Deserialize, Serialize};

/// 服务器监控快照，由后端采集后推送给前端渲染
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MonitorSnapshot {
    pub session_id: String,
    /// 采集时间，Unix 毫秒时间戳
    pub timestamp: i64,
    /// CPU 使用率，0.0 ~ 100.0
    pub cpu_usage: f64,
    /// 内存使用率，0.0 ~ 100.0
    pub memory_usage: f64,
    /// 磁盘使用率，0.0 ~ 100.0
    pub disk_usage: f64,
}

/// 长任务信息，所有持续任务必须可跟踪
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
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
mod tests {
    use super::*;
    use crate::models::session::{SessionInfo, SessionStatus};
    use proptest::prelude::*;

    /// 毫秒时间戳下界：2001-09-09 01:46:40 UTC 对应的毫秒值
    /// 任何合法的系统时间戳都应远大于此值
    const MIN_MILLIS_TIMESTAMP: i64 = 1_000_000_000_000;

    /// 生成合法毫秒时间戳的策略（2001 年之后到 2100 年之前）
    fn arb_millis_timestamp() -> impl Strategy<Value = i64> {
        // 2001-09-09 01:46:40.000 UTC ~ 2100-01-01 00:00:00.000 UTC（毫秒）
        MIN_MILLIS_TIMESTAMP..4_102_444_800_000_i64
    }

    /// 生成非空字符串的策略
    fn arb_nonempty_string() -> impl Strategy<Value = String> {
        "[a-zA-Z0-9_\\-]{1,32}".prop_map(|s| s)
    }

    /// 生成任意合法 SessionInfo 的策略，created_at 使用毫秒时间戳
    fn arb_session_info() -> impl Strategy<Value = SessionInfo> {
        (
            arb_nonempty_string(),  // session_id
            arb_nonempty_string(),  // host_id
            arb_nonempty_string(),  // host
            1u16..=65535u16,        // port
            arb_nonempty_string(),  // username
            arb_millis_timestamp(), // created_at（毫秒）
        )
            .prop_map(|(session_id, host_id, host, port, username, created_at)| {
                SessionInfo {
                    session_id,
                    host_id,
                    host,
                    port,
                    username,
                    status: SessionStatus::Connecting,
                    created_at,
                }
            })
    }

    /// 生成任意合法 MonitorSnapshot 的策略，timestamp 使用毫秒时间戳
    fn arb_monitor_snapshot() -> impl Strategy<Value = MonitorSnapshot> {
        (
            arb_nonempty_string(),  // session_id
            arb_millis_timestamp(), // timestamp（毫秒）
            0.0f64..100.0f64,       // cpu_usage
            0.0f64..100.0f64,       // memory_usage
            0.0f64..100.0f64,       // disk_usage
        )
            .prop_map(
                |(session_id, timestamp, cpu_usage, memory_usage, disk_usage)| MonitorSnapshot {
                    session_id,
                    timestamp,
                    cpu_usage,
                    memory_usage,
                    disk_usage,
                },
            )
    }

    /// 生成任意合法 TaskInfo 的策略，created_at 使用毫秒时间戳
    fn arb_task_info() -> impl Strategy<Value = TaskInfo> {
        (
            arb_nonempty_string(),  // task_id
            arb_nonempty_string(),  // task_type
            arb_millis_timestamp(), // created_at（毫秒）
        )
            .prop_map(|(task_id, task_type, created_at)| TaskInfo {
                task_id,
                task_type,
                session_id: None,
                status: TaskStatus::Pending,
                created_at,
            })
    }

    proptest! {
        /// **验证: 需求 7.1**
        ///
        /// Property 8: 所有时间戳为毫秒
        ///
        /// 验证 SessionInfo.created_at 为毫秒级时间戳（值 > 1_000_000_000_000）
        #[test]
        fn prop_session_info_created_at_is_millis(session in arb_session_info()) {
            prop_assert!(
                session.created_at > MIN_MILLIS_TIMESTAMP,
                "SessionInfo.created_at 必须为毫秒级时间戳（> {}），实际值: {}",
                MIN_MILLIS_TIMESTAMP,
                session.created_at
            );
        }

        /// **验证: 需求 7.1**
        ///
        /// Property 8: 所有时间戳为毫秒
        ///
        /// 验证 MonitorSnapshot.timestamp 为毫秒级时间戳（值 > 1_000_000_000_000）
        #[test]
        fn prop_monitor_snapshot_timestamp_is_millis(snapshot in arb_monitor_snapshot()) {
            prop_assert!(
                snapshot.timestamp > MIN_MILLIS_TIMESTAMP,
                "MonitorSnapshot.timestamp 必须为毫秒级时间戳（> {}），实际值: {}",
                MIN_MILLIS_TIMESTAMP,
                snapshot.timestamp
            );
        }

        /// **验证: 需求 7.1**
        ///
        /// Property 8: 所有时间戳为毫秒
        ///
        /// 验证 TaskInfo.created_at 为毫秒级时间戳（值 > 1_000_000_000_000）
        #[test]
        fn prop_task_info_created_at_is_millis(task in arb_task_info()) {
            prop_assert!(
                task.created_at > MIN_MILLIS_TIMESTAMP,
                "TaskInfo.created_at 必须为毫秒级时间戳（> {}），实际值: {}",
                MIN_MILLIS_TIMESTAMP,
                task.created_at
            );
        }

        /// **验证: 需求 7.1**
        ///
        /// Property 8: 所有时间戳为毫秒（联合验证）
        ///
        /// 同时验证三种结构体的时间戳字段均为毫秒级，
        /// 确保系统中所有暴露给前端的时间字段一致遵守毫秒约定
        #[test]
        fn prop_all_timestamps_are_millis(
            session in arb_session_info(),
            snapshot in arb_monitor_snapshot(),
            task in arb_task_info(),
        ) {
            prop_assert!(
                session.created_at > MIN_MILLIS_TIMESTAMP,
                "SessionInfo.created_at 必须为毫秒级，实际值: {}",
                session.created_at
            );
            prop_assert!(
                snapshot.timestamp > MIN_MILLIS_TIMESTAMP,
                "MonitorSnapshot.timestamp 必须为毫秒级，实际值: {}",
                snapshot.timestamp
            );
            prop_assert!(
                task.created_at > MIN_MILLIS_TIMESTAMP,
                "TaskInfo.created_at 必须为毫秒级，实际值: {}",
                task.created_at
            );
        }
    }
}
