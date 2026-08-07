mod commands;
mod core;
mod errors;
mod models;
mod storage;

use crate::core::monitor_service::MonitorService;
use crate::core::session_manager::SessionManager;
use crate::core::sftp_service::SftpService;
use std::sync::{Arc, Mutex};

/// 初始化并启动 Tauri 应用
///
/// 注册所有插件、全局状态和 invoke 命令处理器，
/// 然后进入 Tauri 事件循环直到应用退出。
pub fn run() {
    let monitor_service = MonitorService::new();
    let sftp_service = Arc::new(Mutex::new(SftpService::new()));

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
