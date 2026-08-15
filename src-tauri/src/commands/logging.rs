use crate::commands::run_blocking_op;
use crate::core::logging::LogStore;
use crate::errors::app_error::{AppError, AppErrorInfo};
use log::LevelFilter;
use std::path::Path;
use tauri::AppHandle;

/** 解析前端传入的日志等级。 */
fn parse_log_level(level: &str) -> Option<LevelFilter> {
    match level {
        "error" => Some(LevelFilter::Error),
        "warn" => Some(LevelFilter::Warn),
        "info" => Some(LevelFilter::Info),
        "debug" => Some(LevelFilter::Debug),
        "trace" => Some(LevelFilter::Trace),
        _ => None,
    }
}

/// 设置运行中日志器的最大输出等级。
#[tauri::command]
pub fn set_log_level(level: String) -> Result<(), AppErrorInfo> {
    let level = parse_log_level(&level).ok_or_else(|| {
        AppErrorInfo::from(AppError::InvalidHostConfig("Invalid log level".to_string()))
    })?;
    log::set_max_level(level);
    Ok(())
}

/// 返回日志文件最近若干行（最新在末尾），供设置面板查看器轮询展示。
///
/// 异步 command：日志文件可达 10MB，全文件读取必须在阻塞线程池执行，
/// 不得占用 Tauri 主线程（查看器每 2 秒轮询一次，主线程阻塞会让后续 invoke 卡死）。
#[tauri::command]
pub async fn get_recent_logs(app: AppHandle) -> Result<Vec<String>, AppErrorInfo> {
    let store = LogStore::new(&app).map_err(AppErrorInfo::from)?;
    run_blocking_op(move || store.read_recent()).await
}

/// 将日志文件复制到用户选择的目标路径（覆盖已存在文件）。
///
/// 异步 command：文件复制走阻塞线程池（与 get_recent_logs 同一主线程不阻塞约定）。
#[tauri::command]
pub async fn export_logs(app: AppHandle, path: String) -> Result<(), AppErrorInfo> {
    let store = LogStore::new(&app).map_err(AppErrorInfo::from)?;
    run_blocking_op(move || store.export_to(Path::new(&path))).await
}

#[cfg(test)]
#[path = "logging_test.rs"]
mod tests;
