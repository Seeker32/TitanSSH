use crate::core::host_identity::HostKeyVerifier;
use crate::core::shared_exec_registry::SharedExecRegistry;
use crate::core::ssh_transport;
use crate::core::ssh_transport::ExecTransport;
use crate::errors::app_error::{AppError, ErrorDetail};
use crate::models::host::HostConfig;
use crate::models::process::{ProcessInfo, ProcessSnapshot};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

/// 进程采集脚本：一次遍历 Linux /proc，并以 base64 字段输出稳定记录。
const PROCESS_SCRIPT: &str = r#"PAGE_SIZE=$(getconf PAGESIZE 2>/dev/null || echo 4096)
CLK_TCK=$(getconf CLK_TCK 2>/dev/null || echo 100)
if [ "$(uname -s 2>/dev/null)" != "Linux" ] || [ ! -r /proc ]; then
  echo "PLATFORM=unsupported"
  exit 0
fi
echo "PLATFORM=linux"
echo "HZ=$CLK_TCK"
b64() { printf '%s' "$1" | base64 | tr -d '\n'; }
for proc in /proc/[0-9]*; do
  [ -r "$proc/stat" ] || continue
  pid=${proc##*/}
  stat_line=$(cat "$proc/stat" 2>/dev/null) || continue
  rest=${stat_line#* }
  command=${rest#\(}
  command=${command%)*}
  fields=${rest##*) }
  set -- $fields
  [ "$#" -ge 22 ] || continue
  state=$1
  ppid=$2
  utime=${12}
  stime=${13}
  rss_pages=${22}
  case "$pid:$ppid:$utime:$stime:$rss_pages" in
    *[!0-9:]*|*:) continue ;;
  esac
  rss_bytes=$((rss_pages * PAGE_SIZE))
  uid=$(awk '/^Uid:/ {print $2; exit}' "$proc/status" 2>/dev/null)
  user=$(getent passwd "$uid" 2>/dev/null | cut -d: -f1)
  [ -n "$user" ] || user=${uid:-unknown}
  command_line=$(tr '\0' ' ' < "$proc/cmdline" 2>/dev/null)
  [ -n "$command_line" ] || command_line=$command
  printf 'P\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
    "$pid" "$ppid" "$state" "$utime" "$stime" "$rss_bytes" \
    "$(b64 "$user")" "$(b64 "$command")" "$(b64 "$command_line")"
done"#;

/// 进程采样循环参数，凭据只在内存中短暂持有。
pub struct ProcessLoopParams {
    /// 主机配置（不含明文凭据）。
    pub host: HostConfig,
    /// 运行时密码。
    pub password: Option<String>,
    /// 运行时私钥口令。
    pub passphrase: Option<String>,
    /// 关联的 Runtime Session ID。
    pub session_id: String,
    /// 置为 true 后停止采样循环。
    pub shutdown: Arc<AtomicBool>,
}

/// 使用注入的连接提供者运行进程采样循环，供生产连接和 mock 共用。
///
/// # 参数
/// - `resolve_fn`: 共享 exec 连接提供者；只在未请求停机时调用一次
/// - `params`: 主机、凭据、会话 ID 与停机标志
/// - `on_snapshot`: 每轮成功采样后的回调
/// - `on_error`: 建连、执行或解析失败后的回调；调用后循环退出
///
/// # 副作用
/// 在当前线程执行远端命令，并每两秒检查一次停机标志。
pub fn run_process_loop_with<ResolveFn>(
    resolve_fn: ResolveFn,
    params: ProcessLoopParams,
    on_snapshot: impl Fn(ProcessSnapshot) + Send + 'static,
    on_error: impl Fn(AppError) + Send + 'static,
) where
    ResolveFn: FnOnce(&HostConfig, Option<&str>, Option<&str>) -> Result<ExecTransport, AppError>,
{
    let ProcessLoopParams {
        host,
        password,
        passphrase,
        session_id,
        shutdown,
    } = params;

    if shutdown.load(Ordering::Relaxed) {
        return;
    }

    let mut transport = match resolve_fn(&host, password.as_deref(), passphrase.as_deref()) {
        Ok(transport) => transport,
        Err(error) => {
            on_error(error);
            return;
        }
    };
    let mut previous_cpu_ticks = HashMap::new();
    let mut previous_timestamp = None;

    while !shutdown.load(Ordering::Relaxed) {
        match collect_once(
            &mut transport,
            &session_id,
            &previous_cpu_ticks,
            previous_timestamp,
        ) {
            Ok((snapshot, current_cpu_ticks)) => {
                previous_cpu_ticks = current_cpu_ticks;
                previous_timestamp = Some(snapshot.timestamp);
                on_snapshot(snapshot);
            }
            Err(error) => {
                on_error(error);
                return;
            }
        }

        for _ in 0..20 {
            if shutdown.load(Ordering::Relaxed) {
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }
    }
}

/// 通过共享 exec 注册表运行生产进程采样循环。
///
/// # 参数
/// - `exec_registry`: 按 sessionId 复用的共享 exec 连接注册表
/// - `verifier`: SSH 主机身份校验器
/// - `params`: 采样循环参数
/// - `on_snapshot`: 成功快照回调
/// - `on_error`: 结构化失败回调
///
/// # 副作用
/// 缺少共享连接时建立 SSH 连接，并在采样失败时结束当前循环。
#[allow(dead_code)]
pub fn run_process_loop(
    exec_registry: SharedExecRegistry,
    verifier: HostKeyVerifier,
    params: ProcessLoopParams,
    on_snapshot: impl Fn(ProcessSnapshot) + Send + 'static,
    on_error: impl Fn(AppError) + Send + 'static,
) {
    let session_key = params.session_id.clone();
    run_process_loop_with(
        move |host, password, passphrase| {
            exec_registry.resolve(&session_key, || {
                ssh_transport::connect_shared_exec(host, password, passphrase, &verifier)
            })
        },
        params,
        on_snapshot,
        on_error,
    );
}

/// 构造 base64 包装的远端采集命令，避免登录 shell 解析脚本元字符。
pub(crate) fn build_collect_command() -> String {
    format!("echo {} | base64 -d | sh", STANDARD.encode(PROCESS_SCRIPT))
}

/// 执行一次远端采集并把输出转换成进程快照。
fn collect_once(
    transport: &mut ExecTransport,
    session_id: &str,
    previous_cpu_ticks: &HashMap<u32, u64>,
    previous_timestamp: Option<i64>,
) -> Result<(ProcessSnapshot, HashMap<u32, u64>), AppError> {
    let output = transport.execute(&build_collect_command())?;
    parse_process_output_at(
        session_id,
        &output,
        previous_cpu_ticks,
        previous_timestamp,
        chrono::Utc::now().timestamp_millis(),
    )
}

/// 解析一轮 /proc 输出，并用相邻采样差值计算当前进程 CPU%。
///
/// # 参数
/// - `session_id`: 快照所属 Runtime Session ID
/// - `output`: 远端采集脚本的完整 stdout
/// - `previous_cpu_ticks`: 上轮仍存活进程的累计 CPU tick
/// - `previous_timestamp`: 上轮快照时间；首轮为 None
/// - `timestamp`: 当前快照的 Unix 毫秒时间戳
///
/// # 返回
/// 当前快照与仅包含本轮进程的 CPU 基线；坏进程记录只丢弃该条。
pub(crate) fn parse_process_output_at(
    session_id: &str,
    output: &str,
    previous_cpu_ticks: &HashMap<u32, u64>,
    previous_timestamp: Option<i64>,
    timestamp: i64,
) -> Result<(ProcessSnapshot, HashMap<u32, u64>), AppError> {
    match output
        .lines()
        .find_map(|line| line.strip_prefix("PLATFORM="))
    {
        Some("linux") => {}
        Some(_) => {
            return Err(AppError::ProcessMonitoringUnsupported(ErrorDetail::msg(
                "进程采样仅支持 Linux 目标主机",
                Vec::new(),
            )));
        }
        None => return Err(process_output_error("缺少远端平台标记")),
    }

    let ticks_per_second = output
        .lines()
        .find_map(|line| line.strip_prefix("HZ=")?.trim().parse::<u64>().ok())
        .filter(|ticks| *ticks > 0)
        .ok_or_else(|| process_output_error("缺少有效的 HZ 采样基准"))?;

    let mut processes = Vec::new();
    let mut current_cpu_ticks = HashMap::new();
    for line in output.lines() {
        let Some(record) = parse_process_record(line) else {
            continue;
        };
        let cpu_ticks = record
            .utime
            .zip(record.stime)
            .and_then(|(utime, stime)| utime.checked_add(stime));
        let cpu_percent = previous_timestamp.and_then(|previous_timestamp| {
            let elapsed_millis = timestamp.checked_sub(previous_timestamp)?;
            let previous_ticks = previous_cpu_ticks.get(&record.pid)?;
            let current_ticks = cpu_ticks?;
            let delta_ticks = current_ticks.checked_sub(*previous_ticks)?;
            (elapsed_millis > 0).then(|| {
                (delta_ticks as f64 * 100_000.0) / (ticks_per_second as f64 * elapsed_millis as f64)
            })
        });

        if let Some(cpu_ticks) = cpu_ticks {
            current_cpu_ticks.insert(record.pid, cpu_ticks);
        }
        processes.push(ProcessInfo {
            pid: record.pid,
            ppid: record.ppid,
            user: record.user,
            command: record.command,
            command_line: record.command_line,
            cpu_percent,
            memory_bytes: record.memory_bytes,
            state: record.state,
        });
    }

    Ok((
        ProcessSnapshot {
            session_id: session_id.to_string(),
            timestamp,
            total_count: processes.len(),
            processes,
        },
        current_cpu_ticks,
    ))
}

/// 已解码的远端进程记录。
struct ProcessRecord {
    pid: u32,
    ppid: u32,
    state: String,
    utime: Option<u64>,
    stime: Option<u64>,
    memory_bytes: Option<u64>,
    user: String,
    command: String,
    command_line: String,
}

/// 解析单行制表符记录；进程竞态导致的坏行只丢弃该进程。
fn parse_process_record(line: &str) -> Option<ProcessRecord> {
    let mut fields = line.split('\t');
    (fields.next()? == "P").then_some(())?;
    let pid = fields.next()?.parse().ok()?;
    let ppid = fields.next()?.parse().ok()?;
    let state = fields.next()?.to_string();
    let utime = fields.next()?.parse().ok();
    let stime = fields.next()?.parse().ok();
    let memory_bytes = fields.next()?.parse().ok();
    let user = decode_field(fields.next()?)?;
    let command = decode_field(fields.next()?)?;
    let command_line = decode_field(fields.next()?)?;
    fields.next().is_none().then_some(ProcessRecord {
        pid,
        ppid,
        state,
        utime,
        stime,
        memory_bytes,
        user,
        command,
        command_line,
    })
}

/// 解码脚本输出的 base64 字段。
fn decode_field(value: &str) -> Option<String> {
    String::from_utf8(STANDARD.decode(value).ok()?).ok()
}

/// 构造稳定的进程采集解析错误。
fn process_output_error(message: &str) -> AppError {
    AppError::ProcessCollectionError(ErrorDetail::msg(message, Vec::new()))
}

#[cfg(test)]
#[path = "process_worker_test.rs"]
mod tests;
