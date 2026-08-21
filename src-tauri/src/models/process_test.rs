#[cfg(test)]
mod tests {
    use crate::models::process::{ProcessInfo, ProcessSnapshot};
    use proptest::prelude::*;
    use serde_json::Value;

    /// 生成满足 wire 约束的随机进程模型。
    fn arb_process_info() -> impl Strategy<Value = ProcessInfo> {
        (
            any::<u32>(),
            any::<u32>(),
            "[a-z0-9_-]{0,12}",
            "[a-z0-9_.-]{0,12}",
            "[a-z0-9 ./_-]{0,24}",
            prop::option::of(0.0f64..1000.0),
            prop::option::of(any::<u64>()),
            "[A-Z?]{1,2}",
        )
            .prop_map(
                |(pid, ppid, user, command, command_line, cpu_percent, memory_bytes, state)| {
                    ProcessInfo {
                        pid,
                        ppid,
                        user,
                        command,
                        command_line,
                        cpu_percent,
                        memory_bytes,
                        state,
                    }
                },
            )
    }

    proptest! {
        /// 进程模型遵守前端 camelCase 序列化契约并携带毫秒时间戳。
        #[test]
        fn process_snapshot_serializes_camel_case(
            process in arb_process_info(),
            session_id in "[a-z0-9-]{1,12}",
            timestamp in 1_700_000_000_000i64..2_000_000_000_000i64,
        ) {
            let snapshot = ProcessSnapshot {
                session_id,
                timestamp,
                processes: vec![process],
                total_count: 1,
            };
            let value: Value = serde_json::to_value(snapshot).unwrap();
            let process = &value["processes"][0];

            prop_assert!(value["sessionId"].is_string());
            prop_assert!(value["timestamp"].as_i64().unwrap() >= 1_700_000_000_000);
            prop_assert!(value.get("session_id").is_none());
            prop_assert!(process["commandLine"].is_string());
            prop_assert!(process.get("command_line").is_none());
            prop_assert!(process["cpuPercent"].is_number() || process["cpuPercent"].is_null());
            prop_assert!(process["memoryBytes"].is_number() || process["memoryBytes"].is_null());
        }
    }
}
