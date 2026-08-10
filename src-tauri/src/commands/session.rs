use crate::core::host_service::HostConfigService;
use crate::core::session_manager::SessionManager;
use crate::errors::app_error::{AppError, AppErrorInfo};
use crate::models::session::SessionInfo;
use tauri::{AppHandle, State};

/// 打开新的 SSH 会话
///
/// 通过 HostConfigService 查询主机配置,传递给 session_manager 协调层,
/// 由 terminal_service 在运行时从 secure_store 读取凭据完成认证。
///
/// # 参数
/// - `app`: Tauri 应用句柄
/// - `host_id`: 目标主机的唯一标识符
/// - `session_manager`: 会话管理器状态
#[tauri::command]
pub fn open_session(
    app: AppHandle,
    host_id: String,
    session_manager: State<'_, SessionManager>,
) -> Result<SessionInfo, AppErrorInfo> {
    // 从持久化存储查询主机配置
    let service = HostConfigService::new(&app).map_err(AppErrorInfo::from)?;
    let host = service
        .get_host(&host_id)
        .map_err(AppErrorInfo::from)?
        .ok_or_else(|| {
            AppErrorInfo::from(AppError::InvalidHostConfig(format!(
                "Host not found: {host_id}"
            )))
        })?;

    // 路由到 session_manager 协调层，由其启动 terminal_service
    session_manager
        .open_session(app, host)
        .map_err(AppErrorInfo::from)
}

/// 关闭指定 SSH 会话
///
/// 通知 session_manager 设置关闭标志并清理会话资源，
/// 同时取消该会话下所有 Pending/Running 的 SFTP 任务。
#[tauri::command]
pub fn close_session(
    app: AppHandle,
    session_id: String,
    session_manager: State<'_, SessionManager>,
) -> Result<(), AppErrorInfo> {
    session_manager
        .close_session(&session_id, &app)
        .map_err(AppErrorInfo::from)
}

/// 向指定会话的终端写入数据
///
/// 将输入数据路由到对应会话的 terminal_service 工作线程。
#[tauri::command]
pub fn write_terminal(
    session_id: String,
    data: String,
    session_manager: State<'_, SessionManager>,
) -> Result<(), AppErrorInfo> {
    session_manager
        .write_terminal(&session_id, data)
        .map_err(AppErrorInfo::from)
}

/// 调整指定会话的终端大小
///
/// 将新的列数和行数路由到对应会话的 terminal_service 工作线程，
/// 由其调用 SSH Channel 的 request_pty_size 同步 PTY 尺寸。
#[tauri::command]
pub fn resize_terminal(
    session_id: String,
    cols: u32,
    rows: u32,
    session_manager: State<'_, SessionManager>,
) -> Result<(), AppErrorInfo> {
    session_manager
        .resize_terminal(&session_id, cols, rows)
        .map_err(AppErrorInfo::from)
}

/// 获取所有活跃会话列表
///
/// 返回 session_manager 内部 HashMap 中所有真实 SSH 会话的 SessionInfo 列表。
/// 状态字段直接来自后端运行时，不依赖前端回写。
#[tauri::command]
pub fn list_sessions(
    session_manager: State<'_, SessionManager>,
) -> Result<Vec<SessionInfo>, AppErrorInfo> {
    Ok(session_manager.list_sessions())
}
