use crate::core::ssh_client;
use crate::errors::app_error::AppError;
use crate::models::host::HostConfig;
use crate::models::monitor::MonitorSnapshot;
use ssh2::Session;
use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

/// 采集脚本：通过 SSH 执行并返回服务器关键指标
const STATUS_SCRIPT: &str = r#"MEMINFO_LINE=$(awk '/MemTotal:/ {total=$2} /MemAvailable:/ {available=$2} END {printf "MEM_TOTAL_KB=%s\nMEM_AVAILABLE_KB=%s\n", total, available}' /proc/meminfo 2>/dev/null)
CPU_LINE=$(awk '/^cpu / {printf "CPU_TOTAL=%s\nCPU_IDLE=%s\n", ($2+$3+$4+$5+$6+$7+$8+$9+$10), ($5+$6)}' /proc/stat 2>/dev/null)
DISK_LINE=$(df -B1 / 2>/dev/null | awk 'NR==2 {gsub(/%/, "", $5); printf "DISK=%s\nDISK_AVAIL=%s\nDISK_TOTAL=%s\n", $5, $4, $2}')
echo "$CPU_LINE"
echo "$MEMINFO_LINE"
echo "$DISK_LINE""#;

/// CPU 原始计数快照，来自 /proc/stat 第一行累计值
type CpuSample = (u64, u64);

/// 监控采集主循环（可注入 connect_fn，便于单元测试）
///
/// 在调用方线程内运行，持有独立 SSH 长连接，每 2 秒采集一次快照。
/// 连接失败或采集出错时调用 on_error 后退出，不自动重连。
/// shutdown=true 时正常退出，不调用 on_error。
///
/// # 参数
/// - `connect_fn`: SSH 连接函数，可注入 mock 供测试使用
/// - `host`: 主机配置（不含明文凭据）
/// - `password`: 运行时密码（Password 认证时必须提供）
/// - `passphrase`: 运行时私钥口令（PrivateKey 认证时可选）
/// - `session_id`: 关联的会话 ID
/// - `shutdown`: 关闭标志，true 时退出循环
/// - `on_snapshot`: 采集成功回调
/// - `on_error`: 采集失败回调，调用后循环退出
pub fn run_monitor_loop_with<ConnFn>(
    connect_fn: ConnFn,
    host: HostConfig,
    password: Option<String>,
    passphrase: Option<String>,
    session_id: String,
    shutdown: Arc<AtomicBool>,
    on_snapshot: impl Fn(MonitorSnapshot) + Send + 'static,
    on_error: impl Fn(AppError) + Send + 'static,
) where
    ConnFn: FnOnce(&HostConfig, Option<&str>, Option<&str>) -> Result<Session, AppError>,
{
    // 保存上一轮 CPU 原始计数，用于根据 /proc/stat 增量计算使用率
    let mut previous_cpu_sample = None;

    // shutdown 预先为 true 时直接退出，不建立连接
    if shutdown.load(Ordering::Relaxed) {
        return;
    }

    // 建立独立 SSH 连接
    let session = match connect_fn(&host, password.as_deref(), passphrase.as_deref()) {
        Ok(s) => s,
        Err(err) => {
            on_error(err);
            return;
        }
    };

    // 采集循环：每 2 秒开新 channel 执行脚本
    while !shutdown.load(Ordering::Relaxed) {
        match collect_once(&session, &session_id, previous_cpu_sample) {
            Ok((snapshot, current_cpu_sample)) => {
                previous_cpu_sample = current_cpu_sample;
                on_snapshot(snapshot);
            }
            Err(err) => {
                on_error(err);
                return;
            }
        }

        // 分 20 次 100ms 检查关闭标志，总计 2 秒间隔
        for _ in 0..20 {
            if shutdown.load(Ordering::Relaxed) {
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }
    }
}

/// 监控采集主循环（使用真实 ssh_client::connect）
///
/// 是 run_monitor_loop_with 的薄包装，生产代码使用此函数。
pub fn run_monitor_loop(
    host: HostConfig,
    password: Option<String>,
    passphrase: Option<String>,
    session_id: String,
    shutdown: Arc<AtomicBool>,
    on_snapshot: impl Fn(MonitorSnapshot) + Send + 'static,
    on_error: impl Fn(AppError) + Send + 'static,
) {
    run_monitor_loop_with(
        |h, pw, pp| ssh_client::connect(h, pw, pp, |_| {}),
        host,
        password,
        passphrase,
        session_id,
        shutdown,
        on_snapshot,
        on_error,
    )
}

/// 通过已建立的 SSH session 执行一次采集，返回 MonitorSnapshot
///
/// 每次调用开新 channel，执行采集脚本，读取输出后关闭 channel。
/// channel 操作失败或 wait_close 失败均返回 AppError。
fn collect_once(
    session: &Session,
    session_id: &str,
    previous_cpu_sample: Option<CpuSample>,
) -> Result<(MonitorSnapshot, Option<CpuSample>), AppError> {
    let mut channel = session.channel_session()?;
    channel.exec(&format!("sh -c '{}'", STATUS_SCRIPT.replace('\'', "'\\''")))?;

    let mut output = String::new();
    channel.read_to_string(&mut output)?;
    channel.wait_close()?;

    parse_snapshot(session_id, &output, previous_cpu_sample)
}

/// 解析脚本输出，构建 MonitorSnapshot
///
/// 解析 KEY=VALUE 格式的脚本输出，提取 /proc/stat、/proc/meminfo 与 df 原始字段。
/// 字段缺失或解析失败时默认回退为 0.0，不返回错误。
///
/// # 参数
/// - `session_id`: 关联的会话 ID
/// - `output`: 脚本标准输出文本
/// - `previous_cpu_sample`: 上一轮 CPU 原始计数，用于计算增量使用率
pub fn parse_snapshot(
    session_id: &str,
    output: &str,
    previous_cpu_sample: Option<CpuSample>,
) -> Result<(MonitorSnapshot, Option<CpuSample>), AppError> {
    let mut memory_total_kb = 0.0_f64;
    let mut memory_available_kb = 0.0_f64;
    let mut disk_usage = 0.0_f64;
    let mut disk_available_bytes = 0_u64;
    let mut disk_total_bytes = 0_u64;
    let mut cpu_total = None;
    let mut cpu_idle = None;

    for line in output.lines() {
        if let Some(v) = line.strip_prefix("CPU_TOTAL=") {
            cpu_total = v.trim().parse().ok();
        } else if let Some(v) = line.strip_prefix("CPU_IDLE=") {
            cpu_idle = v.trim().parse().ok();
        } else if let Some(v) = line.strip_prefix("MEM_TOTAL_KB=") {
            memory_total_kb = v.trim().parse().unwrap_or_default();
        } else if let Some(v) = line.strip_prefix("MEM_AVAILABLE_KB=") {
            memory_available_kb = v.trim().parse().unwrap_or_default();
        } else if let Some(v) = line.strip_prefix("DISK=") {
            disk_usage = v.trim().parse().unwrap_or_default();
        } else if let Some(v) = line.strip_prefix("DISK_AVAIL=") {
            disk_available_bytes = v.trim().parse().unwrap_or_default();
        } else if let Some(v) = line.strip_prefix("DISK_TOTAL=") {
            disk_total_bytes = v.trim().parse().unwrap_or_default();
        }
    }

    let current_cpu_sample = cpu_total.zip(cpu_idle);
    let cpu_usage = compute_cpu_usage(previous_cpu_sample, current_cpu_sample);
    let memory_usage = resolve_memory_usage(memory_total_kb, memory_available_kb);

    Ok((
        MonitorSnapshot {
            session_id: session_id.to_string(),
            timestamp: chrono::Utc::now().timestamp_millis(),
            cpu_usage,
            memory_usage,
            disk_usage,
            disk_available_bytes,
            disk_total_bytes,
        },
        current_cpu_sample,
    ))
}

/// 根据 /proc/stat 连续两次原始计数，计算 CPU 使用率百分比。
///
/// 首轮无基线样本、计数未增长或字段缺失时统一返回 0.0。
fn compute_cpu_usage(
    previous_sample: Option<CpuSample>,
    current_sample: Option<CpuSample>,
) -> f64 {
    let (previous_total, previous_idle) = match previous_sample {
        Some(sample) => sample,
        None => return 0.0,
    };
    let (current_total, current_idle) = match current_sample {
        Some(sample) => sample,
        None => return 0.0,
    };

    let total_delta = current_total.saturating_sub(previous_total);
    let idle_delta = current_idle.saturating_sub(previous_idle);
    if total_delta == 0 {
        return 0.0;
    }

    let busy_ratio = ((total_delta.saturating_sub(idle_delta)) as f64 / total_delta as f64) * 100.0;
    (busy_ratio * 10.0).round() / 10.0
}

/// 根据 /proc/meminfo 的原始字段，计算最终内存使用率。
///
/// MemTotal 缺失或非法时回退为 0.0；MemAvailable 超出总量时按 0 已用处理。
fn resolve_memory_usage(total_kb: f64, available_kb: f64) -> f64 {

    if total_kb <= 0.0 {
        return 0.0;
    }

    let used_ratio = ((total_kb - available_kb).max(0.0) / total_kb) * 100.0;
    (used_ratio * 10.0).round() / 10.0
}

#[cfg(test)]
mod tests {
    use super::{compute_cpu_usage, parse_snapshot};

    /// 验证 parse_snapshot 能正确解析原始脚本输出，并由 Rust 计算内存与磁盘指标
    #[test]
    fn parse_snapshot_extracts_metrics() {
        let raw = "MEM_TOTAL_KB=1000\nMEM_AVAILABLE_KB=577\nDISK=65\nDISK_AVAIL=137438953472\nDISK_TOTAL=549755813888";
        let (snap, cpu_sample) = parse_snapshot("session-1", raw, None).expect("应能正常解析");
        assert_eq!(snap.session_id, "session-1");
        assert_eq!(snap.cpu_usage, 0.0);
        assert!((snap.memory_usage - 42.3).abs() < 0.01);
        assert!((snap.disk_usage - 65.0).abs() < f64::EPSILON);
        assert_eq!(snap.disk_available_bytes, 137_438_953_472);
        assert_eq!(snap.disk_total_bytes, 549_755_813_888);
        assert!(snap.timestamp > 1_000_000_000_000);
        assert_eq!(cpu_sample, None);
    }

    /// 验证 parse_snapshot 在脚本输出为空时不报错，各指标默认为 0.0
    #[test]
    fn parse_snapshot_defaults_on_missing_fields() {
        let (snap, cpu_sample) = parse_snapshot("session-2", "", None).expect("空输出不应返回错误");
        assert_eq!(snap.cpu_usage, 0.0);
        assert_eq!(snap.memory_usage, 0.0);
        assert_eq!(snap.disk_usage, 0.0);
        assert_eq!(snap.disk_available_bytes, 0);
        assert_eq!(snap.disk_total_bytes, 0);
        assert_eq!(cpu_sample, None);
    }

    /// 验证 parse_snapshot 在仅收到内存总量/可用量时，仍能推导出内存使用率
    #[test]
    fn parse_snapshot_computes_memory_usage_from_meminfo_fields() {
        let raw = "MEM_TOTAL_KB=1000\nMEM_AVAILABLE_KB=250\nDISK=65";
        let (snap, _) = parse_snapshot("session-3", raw, None).expect("应能从 meminfo 字段推导内存占用");
        assert!((snap.memory_usage - 75.0).abs() < 0.01);
    }

    /// 验证 parse_snapshot 在有上一轮 CPU 原始计数时能计算本轮 CPU 使用率
    #[test]
    fn parse_snapshot_computes_cpu_usage_from_proc_stat_fields() {
        let raw = "CPU_TOTAL=160\nCPU_IDLE=30\nMEM_TOTAL_KB=1000\nMEM_AVAILABLE_KB=500";
        let (snap, cpu_sample) =
            parse_snapshot("session-4", raw, Some((100, 20))).expect("应能根据 /proc/stat 计数推导 CPU 占用");
        assert!((snap.cpu_usage - 83.3).abs() < 0.01);
        assert_eq!(cpu_sample, Some((160, 30)));
    }

    /// 验证 CPU 使用率会根据两次 /proc/stat 计数增量进行计算
    #[test]
    fn compute_cpu_usage_uses_proc_stat_delta() {
        let usage = compute_cpu_usage(
            Some((100, 20)),
            Some((160, 30)),
        );
        assert!((usage - 83.3).abs() < 0.01);
    }

    /// 验证首轮采样或无效增量时 CPU 使用率回退为 0.0
    #[test]
    fn compute_cpu_usage_defaults_on_missing_or_invalid_delta() {
        assert_eq!(compute_cpu_usage(None, Some((160, 30))), 0.0);
        assert_eq!(compute_cpu_usage(Some((200, 50)), Some((200, 60))), 0.0);
        assert_eq!(compute_cpu_usage(Some((200, 50)), None), 0.0);
    }
}

#[cfg(test)]
mod loop_tests {
    use super::*;
    use crate::errors::app_error::AppError;
    use crate::models::host::{AuthType, HostConfig};
    use crate::models::monitor::MonitorSnapshot;
    use ssh2::Session;
    use std::sync::atomic::AtomicBool;
    use std::sync::{Arc, Mutex};

    /// 构造测试用 HostConfig
    fn make_host() -> HostConfig {
        HostConfig {
            id: "h1".to_string(), name: "test".to_string(),
            host: "127.0.0.1".to_string(), port: 22,
            username: "root".to_string(),
            auth_type: AuthType::Password,
            password_ref: Some("ref".to_string()),
            private_key_path: None, passphrase_ref: None, remark: None,
        }
    }

    /// 连接失败时 on_error 被调用，on_snapshot 不被调用
    #[test]
    fn connect_failure_calls_on_error_not_on_snapshot() {
        let snapshots: Arc<Mutex<Vec<MonitorSnapshot>>> = Arc::new(Mutex::new(vec![]));
        let errors: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(vec![]));
        let shutdown = Arc::new(AtomicBool::new(false));

        let snap_ref = Arc::clone(&snapshots);
        let err_ref = Arc::clone(&errors);

        let connect_fn = |_host: &HostConfig, _pw: Option<&str>, _pp: Option<&str>| {
            Err::<Session, AppError>(AppError::SshConnectionError("mock 连接失败".to_string()))
        };

        run_monitor_loop_with(
            connect_fn,
            make_host(),
            Some("pw".to_string()),
            None,
            "session-1".to_string(),
            shutdown,
            move |snap| { snap_ref.lock().unwrap().push(snap); },
            move |err| { err_ref.lock().unwrap().push(err.to_string()); },
        );

        assert_eq!(snapshots.lock().unwrap().len(), 0, "连接失败时不应调用 on_snapshot");
        assert_eq!(errors.lock().unwrap().len(), 1, "连接失败时应调用一次 on_error");
        assert!(errors.lock().unwrap()[0].contains("mock 连接失败"));
    }

    /// shutdown=true 时循环不执行，on_error 不被调用
    #[test]
    fn shutdown_before_connect_exits_cleanly() {
        let errors: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(vec![]));
        let shutdown = Arc::new(AtomicBool::new(true));

        let err_ref = Arc::clone(&errors);
        let connect_fn = |_: &HostConfig, _: Option<&str>, _: Option<&str>| {
            Err::<Session, AppError>(AppError::SshConnectionError("不应被调用".to_string()))
        };

        run_monitor_loop_with(
            connect_fn,
            make_host(),
            None, None,
            "session-1".to_string(),
            shutdown,
            |_| {},
            move |err| { err_ref.lock().unwrap().push(err.to_string()); },
        );

        assert_eq!(errors.lock().unwrap().len(), 0, "shutdown=true 时不应调用 on_error");
    }
}
