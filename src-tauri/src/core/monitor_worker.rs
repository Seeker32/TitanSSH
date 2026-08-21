use crate::core::host_identity::HostKeyVerifier;
use crate::core::shared_exec_registry::SharedExecRegistry;
use crate::core::ssh_transport;
use crate::core::ssh_transport::ExecTransport;
use crate::errors::app_error::AppError;
use crate::models::host::HostConfig;
use crate::models::monitor::{MonitorSnapshot, NetworkInterface, NetworkSnapshot};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

/// 采集脚本：通过 SSH 执行并返回服务器关键指标
///
/// 末行用 if 块收尾：条件不成立时 POSIX 规定退出码为 0，保证网络源
/// 不可用的主机不因退出码非零被未来的退出码校验误判为采集失败。
const STATUS_SCRIPT: &str = r#"MEMINFO_LINE=$(awk '/MemTotal:/ {total=$2} /MemAvailable:/ {available=$2} END {printf "MEM_TOTAL_KB=%s\nMEM_AVAILABLE_KB=%s\n", total, available}' /proc/meminfo 2>/dev/null)
CPU_LINE=$(awk '/^cpu / {printf "CPU_TOTAL=%s\nCPU_IDLE=%s\n", ($2+$3+$4+$5+$6+$7+$8+$9), ($5+$6)}' /proc/stat 2>/dev/null)
DISK_LINE=$(df -B1 / 2>/dev/null | awk 'NR==2 {gsub(/%/, "", $5); printf "DISK=%s\nDISK_AVAIL=%s\nDISK_TOTAL=%s\n", $5, $4, $2}')
NETWORK_STATUS=unavailable
if [ -r /proc/net/dev ]; then
  NETWORK_LINE=$(awk 'NR > 2 && $1 != "lo:" {printf "NET=%s,%s,%s\n", substr($1, 1, length($1) - 1), $2, $10}' /proc/net/dev 2>/dev/null) && NETWORK_STATUS=available
fi
echo "$CPU_LINE"
echo "$MEMINFO_LINE"
echo "$DISK_LINE"
echo "NETWORK_STATUS=$NETWORK_STATUS"
if [ "$NETWORK_STATUS" = available ]; then
  echo "$NETWORK_LINE"
fi"#;

/// CPU 原始计数快照，来自 /proc/stat 第一行累计值
type CpuSample = (u64, u64);

/// 单次采集的网卡累计计数，用于下次计算传输速率。
#[derive(Debug, Clone, PartialEq)]
struct NetworkSample {
    timestamp: i64,
    interfaces: Vec<NetworkCounter>,
}

/// 单张网卡的原始接收与发送累计字节数。
#[derive(Debug, Clone, PartialEq)]
struct NetworkCounter {
    name: String,
    receive_bytes: u64,
    transmit_bytes: u64,
}

/// 上一轮网卡数量达到此值时建立名称索引；小型主机保持零分配线性查找，
/// 容器/K8s 主机的大量 veth 接口则避免每轮 O(n²) 字符串比较。
const NETWORK_COUNTER_INDEX_THRESHOLD: usize = 8;

#[cfg(test)]
impl NetworkSample {
    /// 构造测试和解析共用的网卡累计计数样本。
    fn new(timestamp: i64, interfaces: Vec<(&str, u64, u64)>) -> Self {
        Self {
            timestamp,
            interfaces: interfaces
                .into_iter()
                .map(|(name, receive_bytes, transmit_bytes)| NetworkCounter {
                    name: name.to_string(),
                    receive_bytes,
                    transmit_bytes,
                })
                .collect(),
        }
    }
}

/// 监控循环的输入参数（不含回调），集中传递避免过长参数列表。
pub struct MonitorLoopParams {
    /// 主机配置（不含明文凭据）
    pub host: HostConfig,
    /// 运行时密码（Password 认证时必须提供）
    pub password: Option<String>,
    /// 运行时私钥口令（PrivateKey 认证时可选）
    pub passphrase: Option<String>,
    /// 关联的会话 ID
    pub session_id: String,
    /// 关闭标志，true 时退出循环
    pub shutdown: Arc<AtomicBool>,
}

/// 监控采集主循环（可注入连接解析函数，便于单元测试）
///
/// 在调用方线程内运行，每 2 秒采集一次快照。
/// 连接解析失败或采集出错时调用 on_error 后退出，不自动重连。
/// params.shutdown 为 true 时正常退出，不调用 on_error。
///
/// # 参数
/// - `resolve_fn`: 共享连接解析函数；生产实现从共享 exec 注册表按 sessionId
///   解析（缺失时建立），测试注入内存实现（不感知注册表存在）
/// - `params`: 循环输入参数（主机配置、运行时凭据、会话 ID、关闭标志）
/// - `on_snapshot`: 采集成功回调
/// - `on_error`: 采集失败回调，调用后循环退出
pub fn run_monitor_loop_with<ResolveFn>(
    resolve_fn: ResolveFn,
    params: MonitorLoopParams,
    on_snapshot: impl Fn(MonitorSnapshot) + Send + 'static,
    on_error: impl Fn(AppError) + Send + 'static,
) where
    ResolveFn: FnOnce(&HostConfig, Option<&str>, Option<&str>) -> Result<ExecTransport, AppError>,
{
    let MonitorLoopParams {
        host,
        password,
        passphrase,
        session_id,
        shutdown,
    } = params;

    // 保存上一轮 CPU 原始计数，用于根据 /proc/stat 增量计算使用率
    let mut previous_cpu_sample = None;
    // 保存上一轮网卡累计计数，用于根据真实采样间隔计算速率。
    let mut previous_network_sample = None;

    // shutdown 预先为 true 时直接退出，不解析连接
    if shutdown.load(Ordering::Relaxed) {
        return;
    }

    // 从注入的解析函数取得共享 SSH 连接
    let mut transport = match resolve_fn(&host, password.as_deref(), passphrase.as_deref()) {
        Ok(transport) => transport,
        Err(err) => {
            on_error(err);
            return;
        }
    };

    // 采集循环：每 2 秒在共享连接上开新 channel 执行脚本
    while !shutdown.load(Ordering::Relaxed) {
        match collect_once(
            &mut transport,
            &session_id,
            previous_cpu_sample,
            previous_network_sample,
        ) {
            Ok((snapshot, current_cpu_sample, current_network_sample)) => {
                previous_cpu_sample = current_cpu_sample;
                previous_network_sample = current_network_sample;
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

/// 监控采集主循环（使用共享 exec 注册表解析连接）
///
/// 是 run_monitor_loop_with 的薄包装，生产代码使用此函数。
/// 连接来源是共享 exec 连接注册表（按 sessionId 键，缺失时建立并插入）：
/// 同一会话的监控与其他采样服务共用一条传输连接，session teardown 时回收。
/// 共享连接与其他 capability 一样经统一主机身份校验：握手后、认证前生效；
/// 连接断开时 execute 失败并经 on_error 上抛（任务侧转 Failed，共享命运）。
pub fn run_monitor_loop(
    exec_registry: SharedExecRegistry,
    verifier: HostKeyVerifier,
    params: MonitorLoopParams,
    on_snapshot: impl Fn(MonitorSnapshot) + Send + 'static,
    on_error: impl Fn(AppError) + Send + 'static,
) {
    let session_key = params.session_id.clone();
    run_monitor_loop_with(
        move |host, password, passphrase| {
            exec_registry.resolve(&session_key, || {
                ssh_transport::connect_shared_exec(host, password, passphrase, &verifier)
            })
        },
        params,
        on_snapshot,
        on_error,
    )
}

/// 通过已建立的 SSH session 执行一次采集，返回 MonitorSnapshot
///
/// 每次调用开新 channel，执行采集脚本，读取输出后关闭 channel。
/// channel 操作失败或 wait_close 失败均返回 AppError。
fn collect_once(
    transport: &mut ExecTransport,
    session_id: &str,
    previous_cpu_sample: Option<CpuSample>,
    previous_network_sample: Option<NetworkSample>,
) -> Result<(MonitorSnapshot, Option<CpuSample>, Option<NetworkSample>), AppError> {
    let output = transport.execute(&build_collect_command(STATUS_SCRIPT))?;

    parse_snapshot_at(
        session_id,
        &output,
        previous_cpu_sample,
        previous_network_sample,
        chrono::Utc::now().timestamp_millis(),
    )
}

/// 将采集脚本编码为可被任意登录 shell（sh/csh/fish）解析的远端命令。
///
/// sshd 用账户登录 shell 的 `-c` 执行 exec 命令，POSIX `'\''` 转义在
/// csh/tcsh/fish 中不成立。改为 `echo <base64> | base64 -d | sh`：base64
/// 字符集不含 shell 元字符，命令无引号无转义，任何 shell 解析结果一致；
/// 脚本原文经 stdin 交给 POSIX sh 执行。目标主机需可用 base64 -d
/// （coreutils/busybox）。
fn build_collect_command(script: &str) -> String {
    format!("echo {} | base64 -d | sh", STANDARD.encode(script))
}

/// 解析单次采集输出并按给定采样时刻计算网卡速率。
///
/// 畸形 NET 行只丢弃该接口（同 lo 跳过），不影响其他指标；仅当
/// NETWORK_STATUS 缺失/不可用时网络区域才整体降级为不可用并重置基线。
/// `previous_network_sample` 仅由监控 worker 传入，用于保留首次采样的未知速率。
fn parse_snapshot_at(
    session_id: &str,
    output: &str,
    previous_cpu_sample: Option<CpuSample>,
    previous_network_sample: Option<NetworkSample>,
    timestamp: i64,
) -> Result<(MonitorSnapshot, Option<CpuSample>, Option<NetworkSample>), AppError> {
    let mut memory_total_kb: Option<f64> = None;
    let mut memory_available_kb: Option<f64> = None;
    let mut disk_usage: Option<f64> = None;
    let mut disk_available_bytes: Option<u64> = None;
    let mut disk_total_bytes: Option<u64> = None;
    let mut cpu_total = None;
    let mut cpu_idle = None;
    let mut network_available = false;
    let mut network_interfaces = vec![];

    // 零指标键的输出说明采集管线本身已损坏（脚本未执行、awk/df 缺失、
    // shell 受限等）：必须报错终止循环，而不是每 2 秒发布一个全 None 的
    // 退化快照。前缀列表与下方解析分支一一对应，新增指标键时需同步。
    const METRIC_KEY_PREFIXES: [&str; 8] = [
        "CPU_TOTAL=",
        "CPU_IDLE=",
        "MEM_TOTAL_KB=",
        "MEM_AVAILABLE_KB=",
        "DISK=",
        "DISK_AVAIL=",
        "DISK_TOTAL=",
        "NET=",
    ];
    let has_any_metric_key = output.lines().any(|line| {
        METRIC_KEY_PREFIXES
            .iter()
            .any(|prefix| line.starts_with(prefix))
    });
    if !has_any_metric_key {
        return Err(AppError::MonitorCollectionError(
            "远端脚本输出未包含任何指标键，采集管线可能已损坏"
                .to_string()
                .into(),
        ));
    }

    for line in output.lines() {
        if let Some(v) = line.strip_prefix("CPU_TOTAL=") {
            cpu_total = v.trim().parse().ok();
        } else if let Some(v) = line.strip_prefix("CPU_IDLE=") {
            cpu_idle = v.trim().parse().ok();
        } else if let Some(v) = line.strip_prefix("MEM_TOTAL_KB=") {
            memory_total_kb = v.trim().parse().ok();
        } else if let Some(v) = line.strip_prefix("MEM_AVAILABLE_KB=") {
            memory_available_kb = v.trim().parse().ok();
        } else if let Some(v) = line.strip_prefix("DISK=") {
            disk_usage = v.trim().parse().ok();
        } else if let Some(v) = line.strip_prefix("DISK_AVAIL=") {
            disk_available_bytes = v.trim().parse().ok();
        } else if let Some(v) = line.strip_prefix("DISK_TOTAL=") {
            disk_total_bytes = v.trim().parse().ok();
        } else if line == "NETWORK_STATUS=available" {
            network_available = true;
        } else if let Some(v) = line.strip_prefix("NET=") {
            // 畸形行与 lo 一样只丢弃该接口，不降级整个网络区域：
            // 接口名异常/输出撕裂只应损失该接口的速率，
            // 只有 NETWORK_STATUS=unavailable 才整体降级并重置基线。
            if v.split(',').next().is_some_and(|name| name.trim() == "lo") {
                continue;
            }
            if let Some(counter) = parse_network_counter(v) {
                network_interfaces.push(counter);
            }
        }
    }

    let current_cpu_sample = cpu_total.zip(cpu_idle);
    let cpu_usage = compute_cpu_usage(previous_cpu_sample, current_cpu_sample);
    let memory_usage = resolve_memory_usage(memory_total_kb, memory_available_kb);
    let (memory_total_bytes, memory_used_bytes) =
        resolve_memory_sizes(memory_total_kb, memory_available_kb);
    let current_network_sample = network_available.then_some(NetworkSample {
        timestamp,
        interfaces: network_interfaces,
    });
    // 基线仅在网络源缺失（NETWORK_STATUS != available）时重置；
    // 个别畸形行已被丢弃，本轮累计计数仍进入基线供下轮计算速率。
    let network = match current_network_sample.as_ref() {
        Some(sample) => NetworkSnapshot {
            available: true,
            interfaces: compute_network_rates(previous_network_sample.as_ref(), sample),
        },
        None => NetworkSnapshot {
            available: false,
            interfaces: vec![],
        },
    };

    Ok((
        MonitorSnapshot {
            session_id: session_id.to_string(),
            timestamp,
            cpu_usage,
            memory_usage,
            memory_total_bytes,
            memory_used_bytes,
            disk_usage,
            disk_available_bytes,
            disk_total_bytes,
            network,
        },
        current_cpu_sample,
        current_network_sample,
    ))
}

/// 解析一行 NET=name,receive,transmit 格式的网卡原始累计计数。
fn parse_network_counter(value: &str) -> Option<NetworkCounter> {
    let mut fields = value.split(',');
    let name = fields.next()?.trim();
    let receive_bytes = fields.next()?.trim().parse().ok()?;
    let transmit_bytes = fields.next()?.trim().parse().ok()?;
    (name != "lo" && !name.is_empty() && fields.next().is_none()).then(|| NetworkCounter {
        name: name.to_string(),
        receive_bytes,
        transmit_bytes,
    })
}

/// 根据当前与上次累计计数及毫秒间隔，生成可序列化的网卡字节每秒速率。
fn compute_network_rates(
    previous: Option<&NetworkSample>,
    current: &NetworkSample,
) -> Vec<NetworkInterface> {
    // 仅在接口数量显著时建立索引：常见主机通常只有少量 NIC，HashMap 分配反而
    // 比线性查找昂贵。entry 保留同名重复记录中的首项，与原 find 语义一致。
    let previous_by_name = previous
        .filter(|sample| sample.interfaces.len() >= NETWORK_COUNTER_INDEX_THRESHOLD)
        .map(|sample| {
            let mut index = HashMap::with_capacity(sample.interfaces.len());
            for counter in &sample.interfaces {
                index.entry(counter.name.as_str()).or_insert(counter);
            }
            index
        });

    current
        .interfaces
        .iter()
        .map(|counter| {
            let previous_counter = previous.and_then(|sample| {
                let candidate = match previous_by_name.as_ref() {
                    Some(index) => index.get(counter.name.as_str()).copied(),
                    None => sample
                        .interfaces
                        .iter()
                        .find(|candidate| candidate.name == counter.name),
                };
                candidate.map(|candidate| (sample, candidate))
            });
            let receive_bytes_per_second = previous_counter.and_then(|(sample, candidate)| {
                rate_between(
                    candidate.receive_bytes,
                    counter.receive_bytes,
                    sample.timestamp,
                    current.timestamp,
                )
            });
            let transmit_bytes_per_second = previous_counter.and_then(|(sample, candidate)| {
                rate_between(
                    candidate.transmit_bytes,
                    counter.transmit_bytes,
                    sample.timestamp,
                    current.timestamp,
                )
            });
            NetworkInterface {
                name: counter.name.clone(),
                receive_bytes_per_second,
                transmit_bytes_per_second,
            }
        })
        .collect()
}

/// 使用真实毫秒间隔计算非负整数速率；回退计数和无效间隔返回未知。
fn rate_between(
    previous: u64,
    current: u64,
    previous_timestamp: i64,
    current_timestamp: i64,
) -> Option<u64> {
    let elapsed_millis = current_timestamp.checked_sub(previous_timestamp)?;
    let delta = current.checked_sub(previous)?;
    (elapsed_millis > 0)
        .then(|| ((delta as u128 * 1_000) / elapsed_millis as u128).min(u64::MAX as u128) as u64)
}

/// 根据 /proc/stat 连续两次原始计数，计算 CPU 使用率百分比。
///
/// 首轮无基线样本或字段缺失时返回 None（未知，与网络首轮 null 语义一致）；
/// 计数未增长视为真实空闲，返回 Some(0.0)。
fn compute_cpu_usage(
    previous_sample: Option<CpuSample>,
    current_sample: Option<CpuSample>,
) -> Option<f64> {
    let (previous_total, previous_idle) = previous_sample?;
    let (current_total, current_idle) = current_sample?;

    let total_delta = current_total.saturating_sub(previous_total);
    let idle_delta = current_idle.saturating_sub(previous_idle);
    if total_delta == 0 {
        return Some(0.0);
    }

    let busy_ratio = ((total_delta.saturating_sub(idle_delta)) as f64 / total_delta as f64) * 100.0;
    Some((busy_ratio * 10.0).round() / 10.0)
}

/// 根据 /proc/meminfo 的原始字段，计算最终内存使用率。
///
/// MemTotal 或 MemAvailable 任一缺失/非法时返回 None（未知），
/// 避免把"未知"误报为 100%（旧内核无 MemAvailable）或 0%。
/// MemAvailable 超出总量时按 0 已用处理。
fn resolve_memory_usage(total_kb: Option<f64>, available_kb: Option<f64>) -> Option<f64> {
    let total = total_kb?;
    if total <= 0.0 {
        return None;
    }
    let available = available_kb?;

    let used_ratio = ((total - available).max(0.0) / total) * 100.0;
    Some((used_ratio * 10.0).round() / 10.0)
}

/// 根据 /proc/meminfo 的原始字段，计算内存总量与已用量的字节表示。
///
/// 未知语义与 `resolve_memory_usage` 完全一致：MemTotal 或 MemAvailable
/// 任一缺失/非法、总量非正时两者均返回 None；MemAvailable 超出总量时
/// 已用钉在 0 字节，不产生负数或 u64 回绕。KB 按四舍五入换算为字节。
fn resolve_memory_sizes(
    total_kb: Option<f64>,
    available_kb: Option<f64>,
) -> (Option<u64>, Option<u64>) {
    match (total_kb, available_kb) {
        (Some(total), Some(available)) if total > 0.0 => {
            let total_bytes = (total * 1024.0).round() as u64;
            let used_bytes = ((total - available).max(0.0) * 1024.0).round() as u64;
            (Some(total_bytes), Some(used_bytes))
        }
        _ => (None, None),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        NetworkSample, STATUS_SCRIPT, build_collect_command, compute_cpu_usage,
        compute_network_rates, parse_snapshot_at,
    };

    /// 验证 parse_snapshot 能正确解析原始脚本输出，并由 Rust 计算内存与磁盘指标
    #[test]
    fn parse_snapshot_extracts_metrics() {
        let raw = "MEM_TOTAL_KB=1000\nMEM_AVAILABLE_KB=577\nDISK=65\nDISK_AVAIL=137438953472\nDISK_TOTAL=549755813888";
        let (snap, cpu_sample, _) =
            parse_snapshot_at("session-1", raw, None, None, 2_000).expect("应能正常解析");
        assert_eq!(snap.session_id, "session-1");
        assert_eq!(snap.cpu_usage, None);
        assert!((snap.memory_usage.unwrap() - 42.3).abs() < 0.01);
        assert_eq!(
            snap.memory_total_bytes,
            Some(1000 * 1024),
            "内存总量应由 MemTotal KB 换算为字节"
        );
        assert_eq!(
            snap.memory_used_bytes,
            Some((1000 - 577) * 1024),
            "内存已用应为 MemTotal-MemAvailable 差值的字节表示"
        );
        assert!((snap.disk_usage.unwrap() - 65.0).abs() < f64::EPSILON);
        assert_eq!(snap.disk_available_bytes, Some(137_438_953_472));
        assert_eq!(snap.disk_total_bytes, Some(549_755_813_888));
        assert_eq!(snap.timestamp, 2_000);
        assert_eq!(cpu_sample, None);
    }

    /// 验证 parse_snapshot 在输出不含任何指标键时返回错误而非全 None 快照：
    /// 零指标键说明采集管线本身已损坏（脚本未执行、awk/df 缺失等），
    /// 必须 surface 为错误终止监控循环；个别字段缺失仍走未知语义。
    #[test]
    fn parse_snapshot_rejects_output_without_any_metric_key() {
        let error =
            parse_snapshot_at("session-2", "", None, None, 2_000).expect_err("空输出应返回错误");
        assert_eq!(error.code(), "MonitorCollectionError");

        // 脚本总会回显 NETWORK_STATUS marker，只有 marker 而无指标键同样是管线损坏
        let error = parse_snapshot_at(
            "session-2",
            "NETWORK_STATUS=unavailable\n",
            None,
            None,
            2_000,
        )
        .expect_err("仅有 marker 的输出应返回错误");
        assert_eq!(error.code(), "MonitorCollectionError");
    }

    /// 验证 MemAvailable 缺失（旧内核）时内存使用率为未知而非误报 100%。
    #[test]
    fn parse_snapshot_reports_unknown_memory_when_available_is_missing() {
        let raw = "MEM_TOTAL_KB=1000\nDISK=65";
        let (snap, _, _) =
            parse_snapshot_at("session-3", raw, None, None, 2_000).expect("应能解析快照");
        assert_eq!(
            snap.memory_usage, None,
            "MemAvailable 缺失时应为未知，不得误报为 100% 已用"
        );
        assert_eq!(
            snap.memory_total_bytes, None,
            "MemAvailable 缺失时内存总量同样未知，不得只报百分比未知"
        );
        assert_eq!(snap.memory_used_bytes, None);
    }

    /// 验证 MemTotal 缺失时内存使用率同样为未知，失败模式与 available 缺失一致。
    #[test]
    fn parse_snapshot_reports_unknown_memory_when_total_is_missing() {
        let raw = "MEM_AVAILABLE_KB=250\nDISK=65";
        let (snap, _, _) =
            parse_snapshot_at("session-3", raw, None, None, 2_000).expect("应能解析快照");
        assert_eq!(snap.memory_usage, None);
        assert_eq!(snap.memory_total_bytes, None);
        assert_eq!(snap.memory_used_bytes, None);
    }

    /// 验证 parse_snapshot 在仅收到内存总量/可用量时，仍能推导出内存使用率
    #[test]
    fn parse_snapshot_computes_memory_usage_from_meminfo_fields() {
        let raw = "MEM_TOTAL_KB=1000\nMEM_AVAILABLE_KB=250\nDISK=65";
        let (snap, _, _) = parse_snapshot_at("session-3", raw, None, None, 2_000)
            .expect("应能从 meminfo 字段推导内存占用");
        assert!((snap.memory_usage.unwrap() - 75.0).abs() < 0.01);
        assert_eq!(snap.memory_total_bytes, Some(1000 * 1024));
        assert_eq!(snap.memory_used_bytes, Some(750 * 1024));
    }

    /// 验证 MemAvailable 超出 MemTotal 时已用字节按 0 钉住，
    /// 与使用率的 clamp 语义一致，不产生负数或回绕值。
    #[test]
    fn parse_snapshot_clamps_used_bytes_to_zero_when_available_exceeds_total() {
        let raw = "MEM_TOTAL_KB=1000\nMEM_AVAILABLE_KB=1500\nDISK=65";
        let (snap, _, _) =
            parse_snapshot_at("session-3", raw, None, None, 2_000).expect("应能解析快照");
        assert_eq!(snap.memory_usage, Some(0.0));
        assert_eq!(snap.memory_total_bytes, Some(1000 * 1024));
        assert_eq!(snap.memory_used_bytes, Some(0));
    }

    /// 验证 parse_snapshot 在有上一轮 CPU 原始计数时能计算本轮 CPU 使用率
    #[test]
    fn parse_snapshot_computes_cpu_usage_from_proc_stat_fields() {
        let raw = "CPU_TOTAL=160\nCPU_IDLE=30\nMEM_TOTAL_KB=1000\nMEM_AVAILABLE_KB=500";
        let (snap, cpu_sample, _) =
            parse_snapshot_at("session-4", raw, Some((100, 20)), None, 2_000)
                .expect("应能根据 /proc/stat 计数推导 CPU 占用");
        assert!((snap.cpu_usage.unwrap() - 83.3).abs() < 0.01);
        assert_eq!(cpu_sample, Some((160, 30)));
    }

    /// 验证 CPU 使用率会根据两次 /proc/stat 计数增量进行计算
    #[test]
    fn compute_cpu_usage_uses_proc_stat_delta() {
        let usage = compute_cpu_usage(Some((100, 20)), Some((160, 30)));
        assert!((usage.unwrap() - 83.3).abs() < 0.01);
    }

    /// 验证首轮采样或无效增量时 CPU 使用率为未知（与网络首轮 null 语义一致）
    #[test]
    fn compute_cpu_usage_defaults_on_missing_or_invalid_delta() {
        assert_eq!(compute_cpu_usage(None, Some((160, 30))), None);
        assert_eq!(
            compute_cpu_usage(Some((200, 50)), Some((200, 60))),
            Some(0.0)
        );
        assert_eq!(compute_cpu_usage(Some((200, 50)), None), None);
    }

    /// 验证远端命令把脚本编码为 base64 管道形式，解码后与脚本原文一致。
    #[test]
    fn collect_command_decodes_to_status_script() {
        use base64::Engine;
        let command = build_collect_command(STATUS_SCRIPT);
        let encoded = command
            .strip_prefix("echo ")
            .and_then(|rest| rest.strip_suffix(" | base64 -d | sh"))
            .expect("命令应为 `echo <b64> | base64 -d | sh` 形式");
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .expect("编码段应为合法 base64");
        assert_eq!(decoded, STATUS_SCRIPT.as_bytes());
    }

    /// 验证命令不含任何引号或 shell 元字符：sshd 用账户登录 shell 执行命令，
    /// csh/tcsh/fish 对 POSIX `'\''` 转义的解析与 sh 不同；base64 字符集
    /// 不含元字符，任何登录 shell 解析结果一致。
    #[test]
    fn collect_command_is_shell_agnostic() {
        let command = build_collect_command(STATUS_SCRIPT);
        assert!(
            !command.contains('\'') && !command.contains('"'),
            "命令不得依赖任何 shell 的引号语义: {command}"
        );
        let encoded = command
            .strip_prefix("echo ")
            .and_then(|rest| rest.strip_suffix(" | base64 -d | sh"))
            .expect("命令应为 `echo <b64> | base64 -d | sh` 形式");
        assert!(
            encoded
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '/' || c == '='),
            "base64 段不得含 shell 元字符: {encoded}"
        );
    }

    /// 验证脚本在网络源不可用时仍以成功状态退出：末行若以失败的
    /// `[ ... ] &&` 结尾，任何校验退出码的调用方都会把无网络的主机
    /// 误判为采集失败（if 块在条件不成立时按 POSIX 以 0 退出）。
    #[test]
    fn status_script_exits_zero_when_network_unavailable() {
        use std::process::Command;

        // 末行 if 块为多行结构，取从最后一个 if 起到结尾的完整语句执行
        let lines: Vec<&str> = STATUS_SCRIPT.lines().collect();
        let start = lines
            .iter()
            .rposition(|line| line.trim_start().starts_with("if "))
            .expect("脚本应以 if 块收尾");
        let final_stmt = lines[start..].join("\n");
        for network_status in ["available", "unavailable"] {
            let status = Command::new("sh")
                .args([
                    "-c",
                    &format!("NETWORK_STATUS={network_status}\n{final_stmt}"),
                ])
                .status()
                .expect("应能执行脚本末行");
            assert!(
                status.success(),
                "NETWORK_STATUS={network_status} 时脚本必须以 0 退出: {final_stmt}"
            );
        }
    }

    /// 验证采集脚本的 CPU 累计不包含 guest 字段：
    /// guest 已计入 user（$2），再加 $10 会造成双重计数（KVM 场景低估使用率）。
    #[test]
    fn status_script_cpu_total_excludes_guest_time() {
        let cpu_line = STATUS_SCRIPT
            .lines()
            .find(|line| line.contains("CPU_TOTAL="))
            .expect("脚本应包含 CPU_TOTAL 采集行");
        assert!(
            cpu_line.contains("($2+$3+$4+$5+$6+$7+$8+$9)"),
            "CPU 累计应只含 $2..$9（guest 已计入 user），实际: {cpu_line}"
        );
        assert!(!cpu_line.contains("$10"), "CPU 累计不得包含 guest($10)");
        assert!(
            !cpu_line.contains("$11"),
            "CPU 累计不得包含 guest_nice($11)"
        );
    }

    /// 验证网络接口保留远端顺序、跳过 lo，并按真实采样间隔计算双向速率。
    #[test]
    fn parse_snapshot_preserves_network_interfaces_and_computes_rates() {
        let raw = "CPU_TOTAL=160\nCPU_IDLE=30\nMEM_TOTAL_KB=1000\nMEM_AVAILABLE_KB=500\nDISK=65\nNETWORK_STATUS=available\nNET=eth1,2000,4000\nNET=eth0,500,1000";
        let previous_network =
            NetworkSample::new(1_000, vec![("eth1", 500, 1_000), ("eth0", 100, 200)]);

        let (snapshot, _, _) =
            parse_snapshot_at("session-1", raw, None, Some(previous_network), 2_000)
                .expect("网络数据有效时应能解析快照");

        assert!(snapshot.network.available);
        assert_eq!(snapshot.network.interfaces[0].name, "eth1");
        assert_eq!(
            snapshot.network.interfaces[0].receive_bytes_per_second,
            Some(1_500)
        );
        assert_eq!(
            snapshot.network.interfaces[0].transmit_bytes_per_second,
            Some(3_000)
        );
        assert_eq!(snapshot.network.interfaces[1].name, "eth0");
        assert_eq!(
            snapshot.network.interfaces[1].receive_bytes_per_second,
            Some(400)
        );
        assert_eq!(
            snapshot.network.interfaces[1].transmit_bytes_per_second,
            Some(800)
        );
    }

    /// 大量网卡时按名称索引上一轮计数，但仍须保留当前轮顺序、正确计算速率，
    /// 并让上轮不存在的新接口维持未知速率。
    #[test]
    fn compute_network_rates_handles_reordered_large_interface_sets() {
        let previous = NetworkSample::new(
            1_000,
            vec![
                ("veth0", 100, 200),
                ("veth1", 200, 400),
                ("veth2", 300, 600),
                ("veth3", 400, 800),
                ("veth4", 500, 1_000),
                ("veth5", 600, 1_200),
                ("veth6", 700, 1_400),
                ("veth7", 800, 1_600),
            ],
        );
        let current = NetworkSample::new(
            2_000,
            vec![
                ("veth7", 1_000, 2_000),
                ("veth0", 150, 300),
                ("veth-new", 1, 1),
            ],
        );

        let rates = compute_network_rates(Some(&previous), &current);

        assert_eq!(rates[0].name, "veth7");
        assert_eq!(rates[0].receive_bytes_per_second, Some(200));
        assert_eq!(rates[0].transmit_bytes_per_second, Some(400));
        assert_eq!(rates[1].name, "veth0");
        assert_eq!(rates[1].receive_bytes_per_second, Some(50));
        assert_eq!(rates[1].transmit_bytes_per_second, Some(100));
        assert_eq!(rates[2].name, "veth-new");
        assert_eq!(rates[2].receive_bytes_per_second, None);
        assert_eq!(rates[2].transmit_bytes_per_second, None);
    }

    /// 验证首次、新接口、计数器回退及无效间隔都保留未知速率，零流量仍为零。
    #[test]
    fn network_rate_edge_cases_remain_distinct_from_zero_traffic() {
        let raw = "NETWORK_STATUS=available\nNET=eth0,100,200\nNET=eth1,300,400";
        let previous_network = NetworkSample::new(2_000, vec![("eth0", 100, 200), ("eth2", 1, 1)]);

        let (snapshot, _, _) =
            parse_snapshot_at("session-1", raw, None, Some(previous_network), 2_000)
                .expect("零间隔不应使整个监控快照失败");

        assert_eq!(
            snapshot.network.interfaces[0].receive_bytes_per_second,
            None
        );
        assert_eq!(
            snapshot.network.interfaces[0].transmit_bytes_per_second,
            None
        );
        assert_eq!(
            snapshot.network.interfaces[1].receive_bytes_per_second,
            None
        );
        assert_eq!(
            snapshot.network.interfaces[1].transmit_bytes_per_second,
            None
        );

        let raw = "NETWORK_STATUS=available\nNET=eth0,100,200";
        let previous_network = NetworkSample::new(1_000, vec![("eth0", 200, 300)]);
        let (snapshot, _, _) =
            parse_snapshot_at("session-1", raw, None, Some(previous_network), 2_000)
                .expect("计数器回退不应使整个监控快照失败");
        assert_eq!(
            snapshot.network.interfaces[0].receive_bytes_per_second,
            None
        );
        assert_eq!(
            snapshot.network.interfaces[0].transmit_bytes_per_second,
            None
        );

        let previous_network = NetworkSample::new(1_000, vec![("eth0", 100, 200)]);
        let (snapshot, _, _) =
            parse_snapshot_at("session-1", raw, None, Some(previous_network), 2_000)
                .expect("零流量不应使整个监控快照失败");
        assert_eq!(
            snapshot.network.interfaces[0].receive_bytes_per_second,
            Some(0)
        );
        assert_eq!(
            snapshot.network.interfaces[0].transmit_bytes_per_second,
            Some(0)
        );
    }

    /// 验证网络源缺失不会阻断 CPU、内存和磁盘快照。
    #[test]
    fn parse_snapshot_keeps_other_metrics_when_network_is_unavailable() {
        let raw = "CPU_TOTAL=160\nCPU_IDLE=30\nMEM_TOTAL_KB=1000\nMEM_AVAILABLE_KB=500\nDISK=65\nDISK_AVAIL=100\nDISK_TOTAL=200";
        let (snapshot, _, network_sample) =
            parse_snapshot_at("session-1", raw, Some((100, 20)), None, 2_000)
                .expect("网络源缺失时仍应产出快照");

        assert!(!snapshot.network.available);
        assert!(snapshot.network.interfaces.is_empty());
        assert_eq!(network_sample, None);
        assert!((snapshot.cpu_usage.unwrap() - 83.3).abs() < 0.01);
        assert!((snapshot.memory_usage.unwrap() - 50.0).abs() < 0.01);
        assert_eq!(snapshot.disk_usage, Some(65.0));
    }

    /// 验证畸形网卡记录只丢弃该行（与 lo 跳过一致），不降级整个网络区域。
    #[test]
    fn parse_snapshot_drops_malformed_network_lines_only() {
        let raw = "CPU_TOTAL=160\nCPU_IDLE=30\nMEM_TOTAL_KB=1000\nMEM_AVAILABLE_KB=500\nDISK=65\nNETWORK_STATUS=available\nNET=eth0,not-a-number,200\nNET=eth1,300,400";
        let previous_network =
            NetworkSample::new(1_000, vec![("eth0", 50, 100), ("eth1", 150, 200)]);

        let (snapshot, _, network_sample) = parse_snapshot_at(
            "session-1",
            raw,
            Some((100, 20)),
            Some(previous_network),
            2_000,
        )
        .expect("畸形网卡记录不应使整个快照失败");

        // 网络区域保持可用：畸形行被丢弃，合法接口照常计算速率
        assert!(snapshot.network.available);
        assert_eq!(snapshot.network.interfaces.len(), 1);
        let eth1 = &snapshot.network.interfaces[0];
        assert_eq!(eth1.name, "eth1");
        assert_eq!(eth1.receive_bytes_per_second, Some(150));
        assert_eq!(eth1.transmit_bytes_per_second, Some(200));
        // 基线必须保留（只含合法接口），供下一轮继续计算速率
        let sample = network_sample.expect("畸形行不得重置网络基线");
        assert_eq!(sample.interfaces.len(), 1);
        assert_eq!(sample.interfaces[0].name, "eth1");
        assert!((snapshot.cpu_usage.unwrap() - 83.3).abs() < 0.01);
        assert!((snapshot.memory_usage.unwrap() - 50.0).abs() < 0.01);
        assert_eq!(snapshot.disk_usage, Some(65.0));
    }

    /// 验证畸形行出现的轮次不重置网络基线：故障消失后的下一轮仍能
    /// 基于上一轮累计计数计算速率，而不是全部退化为 None。
    #[test]
    fn malformed_line_does_not_reset_network_baseline() {
        // 第一轮：eth0 出现畸形行，但合法行必须仍进入基线
        let glitchy = "NETWORK_STATUS=available\nNET=eth0,100,200\nNET=eth0,oops,300";
        let (snapshot, _, baseline) = parse_snapshot_at("session-1", glitchy, None, None, 1_000)
            .expect("畸形行不应使快照失败");
        assert!(snapshot.network.available);
        let baseline = baseline.expect("基线必须保留");
        assert_eq!(baseline.interfaces.len(), 1);
        assert_eq!(baseline.interfaces[0].name, "eth0");

        // 第二轮：故障消失，速率应基于第一轮基线计算
        let clean = "NETWORK_STATUS=available\nNET=eth0,300,600";
        let (snapshot, _, _) = parse_snapshot_at("session-1", clean, None, Some(baseline), 2_000)
            .expect("第二轮应能解析");
        let eth0 = &snapshot.network.interfaces[0];
        assert_eq!(eth0.receive_bytes_per_second, Some(200));
        assert_eq!(eth0.transmit_bytes_per_second, Some(400));
    }

    /// 验证网络采集成功但只有 lo 时仍返回可用的空候选列表。
    #[test]
    fn parse_snapshot_distinguishes_no_network_interfaces_from_unavailable() {
        let raw = "NETWORK_STATUS=available\nNET=lo,100,200";
        let (snapshot, _, _) =
            parse_snapshot_at("session-1", raw, None, None, 2_000).expect("仅有 lo 时仍应产出快照");

        assert!(snapshot.network.available);
        assert!(snapshot.network.interfaces.is_empty());
    }
}

#[cfg(test)]
#[path = "monitor_worker_test.rs"]
mod loop_tests;
