mod commands;
mod core;
mod errors;
mod models;
mod storage;

use crate::core::monitor_service::MonitorService;
use crate::core::session_manager::SessionManager;
use crate::core::sftp_service::SftpService;

/// 初始化控制台日志器，默认输出 info 及以上等级。
fn init_logger() {
    let _ = env_logger::Builder::new()
        .filter_level(log::LevelFilter::Trace)
        .try_init();
    log::set_max_level(log::LevelFilter::Info);
}

/// 初始化并启动 Tauri 应用
///
/// 注册所有插件、全局状态和 invoke 命令处理器，
/// 然后进入 Tauri 事件循环直到应用退出。
pub fn run() {
    init_logger();
    let monitor_service = MonitorService::new();
    let sftp_service = SftpService::new();

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(SessionManager::new(
            monitor_service.clone(),
            sftp_service.clone(),
        ))
        .manage(monitor_service)
        .manage(sftp_service)
        .invoke_handler(tauri::generate_handler![
            commands::host::list_hosts,
            commands::host::save_host,
            commands::host::delete_host,
            commands::logging::set_log_level,
            commands::session::open_session,
            commands::session::close_session,
            commands::session::write_terminal,
            commands::session::resize_terminal,
            commands::session::list_sessions,
            commands::monitor::start_monitoring,
            commands::monitor::stop_monitoring,
            commands::monitor::get_monitor_status,
            commands::sftp::sftp_list_dir,
            commands::sftp::sftp_download,
            commands::sftp::sftp_upload,
            commands::sftp::sftp_cancel_task
        ])
        .run(tauri::generate_context!())
        .expect("error while running Titan SSH");
}

#[cfg(test)]
mod tests {
    use super::init_logger;

    /// 日志器重复初始化不会导致应用启动失败。
    #[test]
    fn logger_initialization_is_idempotent() {
        init_logger();
        init_logger();
    }
}
