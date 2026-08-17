//! 后端日志基础设施：文件落盘 + 查看/导出适配。
//!
//! 全局 log facade 安装一个自定义 Logger：stderr 输出委托 env_logger（保留终端彩色），
//! 同时以纯文本单行追加到 OS 应用日志目录下的 titanssh.log。
//! 查看器读同一文件（单一事实源），导出为文件复制。

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::panic;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::Local;
use log::{Level, LevelFilter, Log, Metadata, Record};
use tauri::{AppHandle, Manager};

use crate::errors::app_error::{AppError, ErrorDetail};

/// 日志文件名（位于 OS 应用日志目录）。
const LOG_FILE_NAME: &str = "titanssh.log";

/// Tauri 配置中的 bundle identifier；早期 panic 时尚无 AppHandle，只能通过该稳定值解析目录。
const APP_IDENTIFIER: &str = "com.titanssh.desktop";

/// 日志文件大小上限；启动时超过即截断，防止无限增长。
/// ponytail: 单文件截断即可，需要滚动轮转时再引入轮转逻辑。
const LOG_MAX_BYTES: u64 = 10 * 1024 * 1024;

/// 查看器最多返回的行数（最新在末尾）。
const LOG_VIEW_MAX_LINES: usize = 500;

/// 尾读窗口字节数：只读取文件末尾这一段，足够覆盖 500 行展示；seek 落在行中间时
/// 至多多读一行长度（不完整前缀被丢弃）。
const LOG_TAIL_BYTES: u64 = 64 * 1024;

/// 解析应用日志目录下 titanssh.log 的完整路径（安装与 LogStore 共用）。
pub fn resolve_log_file_path(app_handle: &AppHandle) -> Result<PathBuf, AppError> {
    let log_dir = app_handle.path().app_log_dir().map_err(|error| {
        AppError::StorageError(ErrorDetail::msg(
            "无法获取应用日志目录: {0}",
            vec![error.to_string()],
        ))
    })?;
    Ok(log_dir.join(LOG_FILE_NAME))
}

/// 在 Tauri Builder 创建前解析 panic 日志文件路径；与 Tauri 的 app_log_dir 平台规则一致。
/// 无法解析 OS 目录时回退到系统临时目录，确保无控制台的 release 构建仍保留诊断文件。
fn early_panic_log_file_path() -> PathBuf {
    #[cfg(target_os = "macos")]
    let log_dir = dirs::home_dir()
        .map(|home| home.join("Library").join("Logs").join(APP_IDENTIFIER))
        .unwrap_or_else(|| std::env::temp_dir().join("TitanSSH").join("logs"));

    #[cfg(not(target_os = "macos"))]
    let log_dir = dirs::data_local_dir()
        .map(|data| data.join(APP_IDENTIFIER).join("logs"))
        .unwrap_or_else(|| std::env::temp_dir().join("TitanSSH").join("logs"));

    log_dir.join(LOG_FILE_NAME)
}

/// 直接追加启动前 panic 诊断；不依赖全局 Logger 或 Tauri AppHandle，调用方负责降级处理。
fn write_early_panic_record(file_path: &Path, message: &str) -> Result<(), std::io::Error> {
    let mut file = ensure_log_file(file_path)?;
    let line = format_entry(
        Level::Error,
        "panic",
        message,
        &Local::now().format("%Y-%m-%d %H:%M:%S%.3f").to_string(),
    );
    writeln!(file, "{line}")?;
    file.flush()
}

/// 安装启动前 panic hook：Builder、context 或插件初始化失败时也写入文件日志。
///
/// 保留先前 hook，调试构建仍可在 stderr 获得标准 panic 输出；写入失败仅尝试 stderr，
/// panic hook 本身绝不因日志失败再次 panic。
pub fn install_early_panic_hook() {
    let previous_hook = panic::take_hook();
    panic::set_hook(Box::new(move |panic_info| {
        let payload = panic_info
            .payload()
            .downcast_ref::<&str>()
            .map(|value| (*value).to_string())
            .or_else(|| panic_info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "non-string panic payload".to_string());
        let location = panic_info
            .location()
            .map(|location| {
                format!(
                    "{}:{}:{}",
                    location.file(),
                    location.line(),
                    location.column()
                )
            })
            .unwrap_or_else(|| "unknown location".to_string());
        let message = format!("{payload} at {location}");
        if let Err(error) = write_early_panic_record(&early_panic_log_file_path(), &message) {
            eprintln!("[TitanSSH] failed to persist early panic diagnostic: {error}");
        }
        previous_hook(panic_info);
    }));
}

/// 日志文件存取适配器：查看器读取与导出复制（写入由全局 Logger 完成）。
pub struct LogStore {
    file_path: PathBuf,
}

impl LogStore {
    /// 通过 Tauri AppHandle 解析日志文件路径（目录由 Logger 安装时创建）。
    pub fn new(app_handle: &AppHandle) -> Result<Self, AppError> {
        Ok(Self {
            file_path: resolve_log_file_path(app_handle)?,
        })
    }

    /// 仅供测试使用：直接通过文件路径构造 LogStore，绕过 AppHandle。
    #[cfg(test)]
    pub(crate) fn from_file_path(file_path: PathBuf) -> Self {
        Self { file_path }
    }

    /// 读取日志文件最后 LOG_VIEW_MAX_LINES 行（最新在末尾）；文件不存在返回空列表。
    ///
    /// seek 尾读：只读取文件末尾 LOG_TAIL_BYTES 窗口并跳过窗口首行的不完整前缀，
    /// 使每 2 秒轮询的 IO 与分配和文件大小解耦（日志文件只增不减，全文件读取
    /// 会随日志增长线性变慢）。
    ///
    /// 按字节读取 + 有损转换：窗口起点是任意字节偏移，落在多字节 UTF-8 字符
    /// 中间（中文日志超过 64 KiB 后必然发生）时 read_to_string 会以 InvalidData
    /// 失败；损失仅发生在随后被丢弃的首行前缀，完整行不受影响。
    pub fn read_recent(&self) -> Result<Vec<String>, AppError> {
        if !self.file_path.exists() {
            return Ok(Vec::new());
        }
        let mut file = File::open(&self.file_path)?;
        let len = file.metadata()?.len();
        let start = len.saturating_sub(LOG_TAIL_BYTES);
        file.seek(SeekFrom::Start(start))?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        let chunk = String::from_utf8_lossy(&bytes);
        // seek 可能落在行中间：丢弃窗口首行的不完整前缀（start == 0 时首行完整，保留）
        let tail = if start > 0 {
            chunk.split_once('\n').map(|(_, rest)| rest).unwrap_or("")
        } else {
            chunk.as_ref()
        };
        let lines: Vec<&str> = tail.lines().collect();
        let keep = if lines.len() > LOG_VIEW_MAX_LINES {
            &lines[lines.len() - LOG_VIEW_MAX_LINES..]
        } else {
            &lines
        };
        Ok(keep.iter().map(|line| (*line).to_string()).collect())
    }

    /// 将日志文件复制到目标路径（覆盖已存在文件）；失败返回 IoError。
    pub fn export_to(&self, target: &Path) -> Result<(), AppError> {
        fs::copy(&self.file_path, target)?;
        Ok(())
    }
}

/// 打开日志文件用于追加；父目录缺失时创建，文件超过 LOG_MAX_BYTES 时截断。
fn ensure_log_file(file_path: &Path) -> Result<File, std::io::Error> {
    if let Some(parent) = file_path.parent() {
        fs::create_dir_all(parent)?;
    }
    if file_path.exists() && file_path.metadata()?.len() > LOG_MAX_BYTES {
        // 超限截断：旧日志整体丢弃后重新记录
        return File::create(file_path);
    }
    OpenOptions::new().create(true).append(true).open(file_path)
}

/// 将一条日志记录格式化为单行纯文本：`2025-06-01 14:30:00.123 [INFO] target: message`。
///
/// 消息内的换行符转义为字面 `\\n` / `\\r`：多行消息（嵌套错误 Debug、
/// 命令输出）不得破坏单行格式，否则查看器出现无归属行、可伪造 [INFO]/[ERROR]
/// 记录（日志注入），且 500 行上限被碎片占满。
fn format_entry(level: Level, target: &str, message: &str, timestamp: &str) -> String {
    let message = message.replace('\n', "\\n").replace('\r', "\\r");
    format!("{timestamp} [{level}] {target}: {message}")
}

/// 全局日志器：stderr 输出委托 env_logger（保留终端彩色与现有行为），
/// 文件输出追加纯文本行；文件写入失败静默忽略（日志不得成为崩溃源）。
/// 全局等级过滤由 log::set_max_level 统一控制，对两个输出同时生效。
struct Logger {
    stderr_logger: env_logger::Logger,
    file: Option<Mutex<File>>,
}

impl Log for Logger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        self.stderr_logger.enabled(metadata)
    }

    fn log(&self, record: &Record) {
        self.stderr_logger.log(record);
        let line = format_entry(
            record.level(),
            record.target(),
            &format!("{}", record.args()),
            &Local::now().format("%Y-%m-%d %H:%M:%S%.3f").to_string(),
        );
        if let Some(Ok(mut file)) = self.file.as_ref().map(|file| file.lock()) {
            let _ = writeln!(file, "{line}");
        }
    }

    fn flush(&self) {
        self.stderr_logger.flush();
        if let Some(Ok(mut file)) = self.file.as_ref().map(|file| file.lock()) {
            let _ = file.flush();
        }
    }
}

/// 为应用安装全局日志器：解析日志目录下的文件路径，失败时退化为仅 stderr 并
/// 在 stderr 留下降级原因（此时日志器尚未安装，stderr 是唯一可用的输出通道，
/// 否则设置面板查看器永远显示为空且无从排查）。
pub fn install_logger_for_app(app_handle: &AppHandle) {
    match resolve_log_file_path(app_handle) {
        Ok(path) => install_logger(Some(&path)),
        Err(error) => {
            eprintln!(
                "[TitanSSH] 无法解析应用日志目录，文件日志已禁用（日志查看器将显示为空）: {error}"
            );
            install_logger(None);
        }
    }
}

/// 记录本模块是否已尝试过安装全局日志器：区分幂等重装（静默）与
/// 首次安装即失败的外部日志器占位（必须 stderr 留下降级诊断）。
static LOGGER_INSTALLED: AtomicBool = AtomicBool::new(false);

/// 安装全局日志器并设置默认等级 info。
///
/// 文件不可用时退化为仅 stderr 输出（应用不因此启动失败）；
/// 已安装过日志器时忽略本次调用（log facade 只允许一个全局日志器，幂等）。
/// 首次安装即失败（其他日志器已占位）时向 stderr 留下降级诊断：
/// 本模块的职责就是提供诊断，绝不能静默丢失全部日志输出。
pub fn install_logger(file_path: Option<&Path>) {
    let file = file_path
        .and_then(|path| ensure_log_file(path).ok())
        .map(Mutex::new);
    let stderr_logger = env_logger::Builder::new()
        .filter_level(LevelFilter::Trace)
        .build();
    let logger = Logger {
        stderr_logger,
        file,
    };
    let install_succeeded = log::set_boxed_logger(Box::new(logger)).is_ok();
    let already_recorded = LOGGER_INSTALLED.swap(true, Ordering::SeqCst);
    if !install_succeeded && !already_recorded {
        eprintln!(
            "[TitanSSH] 全局日志器安装失败：已有其他日志器占用（本应用 stderr 与文件日志均未生效）"
        );
    }
    log::set_max_level(LevelFilter::Info);
}

/// 仅供测试使用：查询是否已尝试过安装全局日志器。
#[cfg(test)]
pub(crate) fn logger_install_recorded() -> bool {
    LOGGER_INSTALLED.load(Ordering::SeqCst)
}

#[cfg(test)]
#[path = "logging_test.rs"]
mod tests;
