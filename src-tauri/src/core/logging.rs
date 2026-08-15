//! 后端日志基础设施：文件落盘 + 查看/导出适配。
//!
//! 全局 log facade 安装一个自定义 Logger：stderr 输出委托 env_logger（保留终端彩色），
//! 同时以纯文本单行追加到 OS 应用日志目录下的 titanssh.log。
//! 查看器读同一文件（单一事实源），导出为文件复制。

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::Local;
use log::{Level, LevelFilter, Log, Metadata, Record};
use tauri::{AppHandle, Manager};

use crate::errors::app_error::AppError;

/// 日志文件名（位于 OS 应用日志目录）。
const LOG_FILE_NAME: &str = "titanssh.log";

/// 日志文件大小上限；启动时超过即截断，防止无限增长。
/// ponytail: 单文件截断即可，需要滚动轮转时再引入轮转逻辑。
const LOG_MAX_BYTES: u64 = 10 * 1024 * 1024;

/// 查看器最多返回的行数（最新在末尾）。
const LOG_VIEW_MAX_LINES: usize = 500;

/// 解析应用日志目录下 titanssh.log 的完整路径（安装与 LogStore 共用）。
pub fn resolve_log_file_path(app_handle: &AppHandle) -> Result<PathBuf, AppError> {
    let log_dir = app_handle
        .path()
        .app_log_dir()
        .map_err(|error| AppError::StorageError(format!("无法获取应用日志目录: {error}")))?;
    Ok(log_dir.join(LOG_FILE_NAME))
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
    /// ponytail: 全文件读取后取尾部；文件超限会被截断，轮询足够快，滞后时改 seek 尾读。
    pub fn read_recent(&self) -> Result<Vec<String>, AppError> {
        if !self.file_path.exists() {
            return Ok(Vec::new());
        }
        let content = fs::read_to_string(&self.file_path)?;
        let lines: Vec<&str> = content.lines().collect();
        let tail = if lines.len() > LOG_VIEW_MAX_LINES {
            &lines[lines.len() - LOG_VIEW_MAX_LINES..]
        } else {
            &lines
        };
        Ok(tail.iter().map(|line| (*line).to_string()).collect())
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
fn format_entry(level: Level, target: &str, message: &str, timestamp: &str) -> String {
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

/// 安装全局日志器并设置默认等级 info。
///
/// 文件不可用时退化为仅 stderr 输出（应用不因此启动失败）；
/// 已安装过日志器时忽略本次调用（log facade 只允许一个全局日志器，幂等）。
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
    let _ = log::set_boxed_logger(Box::new(logger));
    log::set_max_level(LevelFilter::Info);
}

#[cfg(test)]
#[path = "logging_test.rs"]
mod tests;
