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
mod tests {
    use super::{
        LOG_FILE_NAME, LOG_MAX_BYTES, LOG_VIEW_MAX_LINES, LogStore, ensure_log_file, format_entry,
        install_logger,
    };
    use log::Level;
    use std::fs::{self, OpenOptions};
    use std::io::Write;
    use tempfile::tempdir;

    /// 格式化输出固定单行：时间戳 + 等级 + 目标 + 消息。
    #[test]
    fn format_entry_produces_stable_single_line() {
        let line = format_entry(
            Level::Info,
            "core::logging",
            "连接已建立",
            "2025-06-01 14:30:00.123",
        );
        assert_eq!(
            line,
            "2025-06-01 14:30:00.123 [INFO] core::logging: 连接已建立"
        );
    }

    /// 行数低于查看上限时按顺序完整返回。
    #[test]
    fn read_recent_returns_full_content_when_under_limit() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(LOG_FILE_NAME);
        fs::write(&path, "line-1\nline-2\nline-3\n").unwrap();

        let lines = LogStore::from_file_path(path).read_recent().unwrap();
        assert_eq!(lines, vec!["line-1", "line-2", "line-3"]);
    }

    /// 超过上限只返回最后 LOG_VIEW_MAX_LINES 行，最新在末尾。
    #[test]
    fn read_recent_returns_only_last_view_limit_lines_newest_last() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(LOG_FILE_NAME);
        let content: Vec<String> = (0..=LOG_VIEW_MAX_LINES)
            .map(|i| format!("line-{i}"))
            .collect();
        fs::write(&path, content.join("\n")).unwrap();

        let lines = LogStore::from_file_path(path).read_recent().unwrap();
        assert_eq!(lines.len(), LOG_VIEW_MAX_LINES);
        assert_eq!(lines.first().unwrap(), "line-1", "最旧的一行应被丢弃");
        assert_eq!(lines.last().unwrap(), &format!("line-{LOG_VIEW_MAX_LINES}"));
    }

    /// 日志文件不存在时返回空列表（对应首次运行/尚未产生日志）。
    #[test]
    fn read_recent_returns_empty_when_file_missing() {
        let dir = tempdir().unwrap();
        let lines = LogStore::from_file_path(dir.path().join(LOG_FILE_NAME))
            .read_recent()
            .unwrap();
        assert!(lines.is_empty());
    }

    /// 导出复制内容并可覆盖已存在的目标文件。
    #[test]
    fn export_to_copies_content_and_overwrites_existing_target() {
        let dir = tempdir().unwrap();
        let source = dir.path().join(LOG_FILE_NAME);
        fs::write(&source, "debug-1\ndebug-2\n").unwrap();
        let target = dir.path().join("exported.log");
        fs::write(&target, "old").unwrap();

        LogStore::from_file_path(source).export_to(&target).unwrap();
        assert_eq!(fs::read_to_string(&target).unwrap(), "debug-1\ndebug-2\n");
    }

    /// 源日志文件缺失时导出返回稳定 IoError 错误码。
    #[test]
    fn export_to_fails_with_io_error_when_source_missing() {
        let dir = tempdir().unwrap();
        let error = LogStore::from_file_path(dir.path().join(LOG_FILE_NAME))
            .export_to(&dir.path().join("exported.log"))
            .unwrap_err();
        assert_eq!(error.code(), "IoError");
    }

    /// 小文件追加保留、超限文件截断后重新记录。
    #[test]
    fn ensure_log_file_creates_appends_and_truncates_oversized_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(LOG_FILE_NAME);

        let mut file = ensure_log_file(&path).unwrap();
        writeln!(file, "first").unwrap();
        drop(file);
        let mut file = ensure_log_file(&path).unwrap();
        writeln!(file, "second").unwrap();
        drop(file);
        assert_eq!(fs::read_to_string(&path).unwrap(), "first\nsecond\n");

        // set_len 伪造超大文件（稀疏文件，磁盘占用小），验证启动截断
        let file = OpenOptions::new().write(true).open(&path).unwrap();
        file.set_len(LOG_MAX_BYTES + 1).unwrap();
        drop(file);
        let mut file = ensure_log_file(&path).unwrap();
        writeln!(file, "fresh").unwrap();
        drop(file);
        assert_eq!(fs::read_to_string(&path).unwrap(), "fresh\n");
    }

    /// 日志目录不存在时递归创建。
    #[test]
    fn ensure_log_file_creates_missing_parent_directories() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("nested").join("logs").join(LOG_FILE_NAME);
        ensure_log_file(&path).unwrap();
        assert!(path.exists());
    }

    /// 重复安装不 panic（第二次 set_logger 被忽略）；路径缺失时退化为仅 stderr。
    #[test]
    fn install_logger_is_idempotent_and_falls_back_to_stderr_only() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(LOG_FILE_NAME);
        install_logger(Some(&path));
        install_logger(Some(&path));
        install_logger(None);
    }
}
