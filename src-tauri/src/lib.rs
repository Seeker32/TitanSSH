mod commands;
mod core;
mod errors;
mod models;
mod storage;

use crate::core::host_identity::HostIdentityService;
use crate::core::monitor_service::MonitorService;
use crate::core::session_manager::SessionManager;
use crate::core::sftp_service::SftpService;
use tauri::Manager;

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
    let identity_service = HostIdentityService::new();

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            // 初始化 TitanSSH 独立信任存储（应用数据目录下的 known_hosts）；
            // 不读取系统 ~/.ssh/known_hosts，也不使用 keyring
            app.state::<HostIdentityService>()
                .init_trust_store(app.handle())?;
            Ok(())
        })
        .manage(SessionManager::new(
            monitor_service.clone(),
            sftp_service.clone(),
            identity_service.clone(),
        ))
        .manage(monitor_service)
        .manage(sftp_service)
        .manage(identity_service)
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
            commands::host_identity::accept_host_identity,
            commands::host_identity::accept_and_save_host_identity,
            commands::host_identity::reject_host_identity,
            commands::monitor::start_monitoring,
            commands::monitor::stop_monitoring,
            commands::monitor::get_monitor_status,
            commands::sftp::sftp_list_dir,
            commands::sftp::sftp_download,
            commands::sftp::sftp_upload,
            commands::sftp::sftp_cancel_task,
            commands::sftp::sftp_task_snapshot,
            commands::sftp::sftp_clear_terminal_tasks
        ])
        .build(tauri::generate_context!())
        .expect("error while building Titan SSH")
        .run(|app_handle, event| {
            // 应用退出：取消全部主机身份等待者，等待中的连接不进入认证
            if let tauri::RunEvent::Exit = event {
                app_handle.state::<HostIdentityService>().cancel_all();
            }
        });
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
