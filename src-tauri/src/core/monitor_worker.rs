use crate::core::ssh_client;
use crate::errors::app_error::AppError;
use crate::models::host::HostConfig;
use crate::models::monitor::MonitorSnapshot;
use std::io::Read;

/// 采集脚本：通过 SSH 执行并返回服务器关键指标
const STATUS_SCRIPT: &str = r#"
MEM=$(free 2>/dev/null | awk '/Mem:/ {printf "%.1f", $3/$2*100}')
CPU=$(top -bn1 2>/dev/null | awk -F'[, ]+' '/Cpu/ {print 100-$8}')
DISK=$(df / 2>/dev/null | awk 'NR==2 {print $5}' | tr -d '%')
echo "CPU=$CPU"
echo "MEM=$MEM"
echo "DISK=$DISK"
"#;

/// 通过 SSH 采集服务器监控快照，返回结构化 MonitorSnapshot
/// - host: 主机配置，用于建立 SSH 连接
/// - session_id: 关联的会话 ID
pub fn collect_status(host: &HostConfig, session_id: &str) -> Result<MonitorSnapshot, AppError> {
    let session = ssh_client::connect(host, None, None, |_| {})?;
    let mut channel = session.channel_session()?;
    channel.exec(&format!(
        "sh -lc '{}'",
        STATUS_SCRIPT.replace('\'', "'\\''")
    ))?;

    let mut output = String::new();
    channel.read_to_string(&mut output)?;
    channel.wait_close()?;

    parse_snapshot(session_id, &output)
}

/// 解析脚本输出，构建 MonitorSnapshot
/// - session_id: 关联的会话 ID
/// - output: 脚本标准输出文本
fn parse_snapshot(session_id: &str, output: &str) -> Result<MonitorSnapshot, AppError> {
    let mut cpu_usage = 0.0_f64;
    let mut memory_usage = 0.0_f64;
    let mut disk_usage = 0.0_f64;

    for line in output.lines() {
        if let Some(v) = line.strip_prefix("CPU=") {
            cpu_usage = v.trim().parse().unwrap_or_default();
        } else if let Some(v) = line.strip_prefix("MEM=") {
            memory_usage = v.trim().parse().unwrap_or_default();
        } else if let Some(v) = line.strip_prefix("DISK=") {
            disk_usage = v.trim().parse().unwrap_or_default();
        }
    }

    Ok(MonitorSnapshot {
        session_id: session_id.to_string(),
        timestamp: chrono::Utc::now().timestamp_millis(),
        cpu_usage,
        memory_usage,
        disk_usage,
    })
}

#[cfg(test)]
mod tests {
    use super::parse_snapshot;

    /// 验证 parse_snapshot 能正确解析标准脚本输出，提取 CPU/内存/磁盘指标
    #[test]
    fn parse_snapshot_extracts_metrics() {
        let raw = "CPU=17.5\nMEM=42.3\nDISK=65";
        let snap = parse_snapshot("session-1", raw).expect("应能正常解析");
        assert_eq!(snap.session_id, "session-1");
        assert!((snap.cpu_usage - 17.5).abs() < f64::EPSILON);
        assert!((snap.memory_usage - 42.3).abs() < 0.01);
        assert!((snap.disk_usage - 65.0).abs() < f64::EPSILON);
        // 时间戳应为毫秒级（大于 1 万亿）
        assert!(snap.timestamp > 1_000_000_000_000);
    }

    /// 验证 parse_snapshot 在脚本输出为空时不报错，各指标默认为 0.0
    #[test]
    fn parse_snapshot_defaults_on_missing_fields() {
        let snap = parse_snapshot("session-2", "").expect("空输出不应返回错误");
        assert_eq!(snap.cpu_usage, 0.0);
        assert_eq!(snap.memory_usage, 0.0);
        assert_eq!(snap.disk_usage, 0.0);
    }
}
