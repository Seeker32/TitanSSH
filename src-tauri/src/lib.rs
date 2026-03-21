mod commands;
mod core;
mod errors;
mod models;
mod storage;

use crate::core::session_manager::SessionManager;
use std::sync::Mutex;

/// 初始化并启动 Tauri 应用
///
/// 注册所有插件、全局状态和 invoke 命令处理器，
/// 然后进入 Tauri 事件循环直到应用退出。
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .manage(Mutex::new(SessionManager::new()))
        .invoke_handler(tauri::generate_handler![
            commands::host::list_hosts,
            commands::host::save_host,
            commands::host::delete_host,
            commands::session::open_session,
            commands::session::close_session,
            commands::session::write_terminal,
            commands::session::resize_terminal,
            commands::session::list_sessions,
            commands::session::sync_session_status,
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
