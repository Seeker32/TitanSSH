mod commands;
mod core;
mod errors;
mod models;
mod storage;

use crate::core::host_identity::HostIdentityService;
use crate::core::host_service::SharedHostConfigService;
use crate::core::logging;
use crate::core::monitor_service::MonitorService;
use crate::core::session_manager::SessionManager;
use crate::core::sftp_service::SftpService;
use tauri::Manager;

/// 安装 Tauri 初始化前的 panic 文件日志 hook，供二进制入口在启动第一步调用。
pub fn install_early_panic_hook() {
    logging::install_early_panic_hook();
}

/// 初始化并启动 Tauri 应用
///
/// 注册所有插件、全局状态和 invoke 命令处理器，
/// 然后进入 Tauri 事件循环直到应用退出。
pub fn run() {
    let monitor_service = MonitorService::new();
    let sftp_service = SftpService::new();
    let identity_service = HostIdentityService::new();

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            // 尽早安装全局日志器：写入 OS 应用日志目录下的 titanssh.log；
            // 日志目录不可用则退化为仅 stderr，不阻断启动
            logging::install_logger_for_app(app.handle());
            // 初始化 TitanSSH 独立信任存储（应用数据目录下的 known_hosts）；
            // 不读取系统 ~/.ssh/known_hosts，也不使用 keyring
            app.state::<HostIdentityService>()
                .init_trust_store(app.handle())?;
            // 受管共享主机服务：所有 host 命令复用同一实例并持锁串行化
            // hosts.json 的 load-modify-write 周期，防止并发 invoke 互相覆盖
            app.manage(SharedHostConfigService::new(app.handle())?);
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
            commands::logging::get_recent_logs,
            commands::logging::export_logs,
            commands::session::open_session,
            commands::session::close_session,
            commands::session::write_terminal,
            commands::session::resize_terminal,
            commands::session::list_sessions,
            commands::host_identity::accept_host_identity,
            commands::host_identity::accept_and_save_host_identity,
            commands::host_identity::reject_host_identity,
            commands::host_identity::list_trusted_hosts,
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
            // 退出请求和最终退出均执行幂等的全量回收；前者让 worker 尽早收到关闭信号。
            if matches!(
                event,
                tauri::RunEvent::ExitRequested { .. } | tauri::RunEvent::Exit
            ) {
                app_handle
                    .state::<SessionManager>()
                    .shutdown_all(app_handle);
            }
        });
}
