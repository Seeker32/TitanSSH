use crate::commands::run_blocking_op;
use crate::core::logging::LogStore;
use crate::errors::app_error::{AppError, AppErrorInfo};
use log::LevelFilter;
use tauri::AppHandle;
use tauri_plugin_dialog::DialogExt;

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

/// 导出默认文件名：titanssh-<yyyy-mm-dd_hh-mm-ss>.log，便于按时间归档。
fn default_export_name() -> String {
    format!(
        "titanssh-{}.log",
        chrono::Local::now().format("%Y-%m-%d_%H-%M-%S")
    )
}

/// 弹出保存对话框并把日志文件复制到用户选择的目标路径。
///
/// 目标路径由后端对话框解析，绝不经过 IPC 边界：webview 内任意脚本（含渲染远端
/// 终端输出场景下的 XSS）无法指定任意路径覆盖用户文件；已有文件由 OS 保存对话框
/// 负责覆盖确认。用户取消视为成功返回（不是错误）。对话框与文件复制均在阻塞线程
/// 池执行，不占用 Tauri 主线程。
#[tauri::command]
pub async fn export_logs(app: AppHandle) -> Result<(), AppErrorInfo> {
    let store = LogStore::new(&app).map_err(AppErrorInfo::from)?;
    run_blocking_op(move || {
        let Some(target) = app
            .dialog()
            .file()
            .set_file_name(default_export_name())
            .blocking_save_file()
            .and_then(|file| file.into_path().ok())
        else {
            return Ok(());
        };
        store.export_to(&target)
    })
    .await
}

#[cfg(test)]
#[path = "logging_test.rs"]
mod tests;
