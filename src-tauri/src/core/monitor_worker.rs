use crate::core::ssh_client;
use crate::errors::app_error::AppError;
use crate::models::host::HostConfig;
use crate::models::monitor::ServerStatus;
use std::io::Read;

const STATUS_SCRIPT: &str = r#"
IP=$(hostname -I 2>/dev/null | awk '{print $1}')
if [ -z "$IP" ]; then
  IP=$(hostname 2>/dev/null)
fi
UPTIME=$(cut -d. -f1 /proc/uptime 2>/dev/null)
LOAD=$(cut -d' ' -f1-3 /proc/loadavg 2>/dev/null)
MEM=$(free -m 2>/dev/null | awk '/Mem:/ {print $3","$2}')
SWAP=$(free -m 2>/dev/null | awk '/Swap:/ {print $3","$2}')
CPU=$(top -bn1 2>/dev/null | awk -F'[, ]+' '/Cpu/ {print 100-$8}')
echo "IP=$IP"
echo "UPTIME=$UPTIME"
echo "LOAD=$LOAD"
echo "MEM=$MEM"
echo "SWAP=$SWAP"
echo "CPU=$CPU"
"#;

pub fn collect_status(host: &HostConfig, session_id: &str) -> Result<ServerStatus, AppError> {
    let session = ssh_client::connect(host)?;
    let mut channel = session.channel_session()?;
    channel.exec(&format!("sh -lc '{}'", STATUS_SCRIPT.replace('\'', "'\\''")))?;

    let mut output = String::new();
    channel.read_to_string(&mut output)?;
    channel.wait_close()?;

    parse_status_output(session_id, &host.host, &output)
}

fn parse_status_output(
    session_id: &str,
    fallback_ip: &str,
    output: &str,
) -> Result<ServerStatus, AppError> {
    let mut ip = fallback_ip.to_string();
    let mut uptime_seconds = 0_u64;
    let mut load = [0.0_f32; 3];
    let mut cpu_percent = 0.0_f32;
    let mut memory = (0_u64, 0_u64);
    let mut swap = (0_u64, 0_u64);

    for line in output.lines() {
        if let Some(value) = line.strip_prefix("IP=") {
            if !value.trim().is_empty() {
                ip = value.trim().to_string();
            }
        } else if let Some(value) = line.strip_prefix("UPTIME=") {
            uptime_seconds = value.trim().parse().unwrap_or_default();
        } else if let Some(value) = line.strip_prefix("LOAD=") {
            let parts: Vec<f32> = value
                .split_whitespace()
                .filter_map(|item| item.parse::<f32>().ok())
                .collect();
            if parts.len() == 3 {
                load.copy_from_slice(&parts[..3]);
            }
        } else if let Some(value) = line.strip_prefix("MEM=") {
            memory = parse_pair(value)?;
        } else if let Some(value) = line.strip_prefix("SWAP=") {
            swap = parse_pair(value)?;
        } else if let Some(value) = line.strip_prefix("CPU=") {
            cpu_percent = value.trim().parse().unwrap_or_default();
        }
    }

    let updated_at = chrono::Utc::now().timestamp();
    Ok(ServerStatus {
        session_id: session_id.to_string(),
        ip,
        uptime_text: humanize_uptime(uptime_seconds),
        load1: load[0],
        load5: load[1],
        load15: load[2],
        cpu_percent,
        memory_used_mb: memory.0,
        memory_total_mb: memory.1,
        memory_percent: percent(memory.0, memory.1),
        swap_used_mb: swap.0,
        swap_total_mb: swap.1,
        swap_percent: percent(swap.0, swap.1),
        updated_at,
    })
}

fn parse_pair(raw: &str) -> Result<(u64, u64), AppError> {
    let mut parts = raw.trim().split(',');
    let used = parts
        .next()
        .unwrap_or_default()
        .trim()
        .parse()
        .map_err(|_| AppError::SshConnectionError(format!("Failed to parse metric: {raw}")))?;
    let total = parts
        .next()
        .unwrap_or_default()
        .trim()
        .parse()
        .map_err(|_| AppError::SshConnectionError(format!("Failed to parse metric: {raw}")))?;
    Ok((used, total))
}

fn percent(used: u64, total: u64) -> f32 {
    if total == 0 {
        0.0
    } else {
        (used as f32 / total as f32) * 100.0
    }
}

fn humanize_uptime(seconds: u64) -> String {
    let days = seconds / 86_400;
    let hours = (seconds % 86_400) / 3_600;
    let minutes = (seconds % 3_600) / 60;

    if days > 0 {
        format!("{days}d {hours}h {minutes}m")
    } else if hours > 0 {
        format!("{hours}h {minutes}m")
    } else {
        format!("{minutes}m")
    }
}

#[cfg(test)]
mod tests {
    use super::{humanize_uptime, parse_pair, parse_status_output, percent};

    #[test]
    fn parse_status_output_builds_server_status() {
        let raw = "\
IP=172.16.0.3
UPTIME=93784
LOAD=0.52 0.41 0.36
MEM=512,2048
SWAP=0,1024
CPU=17.5";

        let status = parse_status_output("session-1", "fallback-host", raw)
            .expect("status output should parse");

        assert_eq!(status.session_id, "session-1");
        assert_eq!(status.ip, "172.16.0.3");
        assert_eq!(status.uptime_text, "1d 2h 3m");
        assert!((status.load1 - 0.52).abs() < f32::EPSILON);
        assert!((status.memory_percent - 25.0).abs() < f32::EPSILON);
        assert_eq!(status.swap_percent, 0.0);
        assert!((status.cpu_percent - 17.5).abs() < f32::EPSILON);
    }

    #[test]
    fn parse_status_output_uses_fallback_ip_when_missing() {
        let status = parse_status_output("session-2", "fallback-host", "UPTIME=120")
            .expect("minimal output should parse");
        assert_eq!(status.ip, "fallback-host");
        assert_eq!(status.uptime_text, "2m");
    }

    #[test]
    fn parse_pair_rejects_invalid_metrics() {
        let error = parse_pair("broken-value").expect_err("invalid metric should fail");
        assert!(error.to_string().contains("Failed to parse metric"));
    }

    #[test]
    fn percent_handles_zero_total() {
        assert_eq!(percent(10, 0), 0.0);
        assert_eq!(percent(10, 40), 25.0);
    }

    #[test]
    fn humanize_uptime_formats_hours_and_minutes() {
        assert_eq!(humanize_uptime(3_660), "1h 1m");
        assert_eq!(humanize_uptime(59), "0m");
    }
}
