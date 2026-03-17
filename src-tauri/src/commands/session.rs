use crate::core::session_manager::SessionManager;
use crate::models::session::{SessionInfo, SessionStatus};
use crate::storage::host_store::HostStore;
use std::sync::Mutex;
use tauri::{AppHandle, State};

/// 打开新的 SSH 会话
///
/// 从 host_store 加载主机配置，传递给 session_manager 协调层，
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
    session_manager: State<'_, Mutex<SessionManager>>,
) -> Result<SessionInfo, String> {
    // 从持久化存储加载主机配置
    let store = HostStore::new(&app)?;
    let hosts = store.load()?;
    let host = hosts
        .into_iter()
        .find(|item| item.id == host_id)
        .ok_or_else(|| format!("Host not found: {host_id}"))?;

    // 路由到 session_manager 协调层，由其启动 terminal_service
    session_manager
        .lock()
        .map_err(|error| error.to_string())?
        .open_session(app, host)
        .map_err(String::from)
}

/// 关闭指定 SSH 会话
///
/// 通知 session_manager 设置关闭标志并清理会话资源。
#[tauri::command]
pub fn close_session(
    session_id: String,
    session_manager: State<'_, Mutex<SessionManager>>,
) -> Result<(), String> {
    session_manager
        .lock()
        .map_err(|error| error.to_string())?
        .close_session(&session_id)
        .map_err(String::from)
}

/// 向指定会话的终端写入数据
///
/// 将输入数据路由到对应会话的 terminal_service 工作线程。
#[tauri::command]
pub fn write_terminal(
    session_id: String,
    data: String,
    session_manager: State<'_, Mutex<SessionManager>>,
) -> Result<(), String> {
    session_manager
        .lock()
        .map_err(|error| error.to_string())?
        .write_terminal(&session_id, data)
        .map_err(String::from)
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
    session_manager: State<'_, Mutex<SessionManager>>,
) -> Result<(), String> {
    session_manager
        .lock()
        .map_err(|error| error.to_string())?
        .resize_terminal(&session_id, cols, rows)
        .map_err(String::from)
}

/// 获取所有活跃会话列表
///
/// 返回 session_manager 内部 HashMap 中所有真实 SSH 会话的 SessionInfo 列表。
/// 状态字段已通过 sync_session_status 保持与实际运行态一致。
#[tauri::command]
pub fn list_sessions(
    session_manager: State<'_, Mutex<SessionManager>>,
) -> Result<Vec<SessionInfo>, String> {
    Ok(session_manager
        .lock()
        .map_err(|error| error.to_string())?
        .list_sessions())
}

/// 同步会话状态到后端元数据
///
/// 前端收到 session:status 事件后调用此命令，将状态变更写回 SessionManager 的 HashMap，
/// 确保 list_sessions 返回的状态与实际运行态一致（修复 P1-1）。
///
/// # 参数
/// - `session_id`: 会话唯一标识符
/// - `status`: 新的会话状态
#[tauri::command]
pub fn sync_session_status(
    session_id: String,
    status: SessionStatus,
    session_manager: State<'_, Mutex<SessionManager>>,
) -> Result<(), String> {
    session_manager
        .lock()
        .map_err(|error| error.to_string())?
        .update_session_status(&session_id, status);
    Ok(())
}
