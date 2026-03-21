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
const STATUS_SCRIPT: &str = r#"MEM=$(free 2>/dev/null | awk '/Mem:/ {printf "%.1f", $3/$2*100}')
CPU=$(top -bn1 2>/dev/null | awk -F'[, ]+' '/Cpu/ {print 100-$8}')
DISK=$(df / 2>/dev/null | awk 'NR==2 {print $5}' | tr -d '%')
echo "CPU=$CPU"
echo "MEM=$MEM"
echo "DISK=$DISK""#;

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
        match collect_once(&session, &session_id) {
            Ok(snapshot) => on_snapshot(snapshot),
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
fn collect_once(session: &Session, session_id: &str) -> Result<MonitorSnapshot, AppError> {
    let mut channel = session.channel_session()?;
    channel.exec(&format!("sh -c '{}'", STATUS_SCRIPT.replace('\'', "'\\''")))?;

    let mut output = String::new();
    channel.read_to_string(&mut output)?;
    channel.wait_close()?;

    parse_snapshot(session_id, &output)
}

/// 解析脚本输出，构建 MonitorSnapshot
///
/// 解析 KEY=VALUE 格式的脚本输出，提取 CPU/内存/磁盘指标。
/// 字段缺失或解析失败时默认为 0.0，不返回错误。
///
/// # 参数
/// - `session_id`: 关联的会话 ID
/// - `output`: 脚本标准输出文本
pub fn parse_snapshot(session_id: &str, output: &str) -> Result<MonitorSnapshot, AppError> {
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

#[cfg(test)]
mod loop_tests {
    use super::*;
    use crate::errors::app_error::AppError;
    use crate::models::host::{AuthType, HostConfig};
    use crate::models::monitor::MonitorSnapshot;
    use ssh2::Session;
    use std::sync::atomic::{AtomicBool, Ordering};
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
