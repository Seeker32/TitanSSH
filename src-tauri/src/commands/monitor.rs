use crate::core::session_manager::SessionManager;
use crate::models::monitor::{MonitorSnapshot, TaskInfo};
use std::sync::Mutex;
use tauri::{AppHandle, State};

/// 为指定会话启动监控任务
///
/// 委托给 session_manager，由其读取凭据并调用 monitor_service 创建后台采集任务。
/// 返回包含 task_id 的 TaskInfo，前端可用于跟踪任务状态。
/// 凭据读取失败或 session 不存在时返回错误字符串。
#[tauri::command]
pub fn start_monitoring(
    app: AppHandle,
    session_id: String,
    session_manager: State<'_, Mutex<SessionManager>>,
) -> Result<TaskInfo, String> {
    session_manager
        .lock()
        .map_err(|error| error.to_string())?
        .start_monitoring(session_id, app)
        .map_err(|error| error.to_string())
}

/// 停止指定 task_id 对应的监控任务
///
/// 委托给 session_manager，由其调用 monitor_service 设置关闭标志并清理任务句柄。
#[tauri::command]
pub fn stop_monitoring(
    task_id: String,
    session_manager: State<'_, Mutex<SessionManager>>,
) -> Result<(), String> {
    session_manager
        .lock()
        .map_err(|error| error.to_string())?
        .stop_monitoring(&task_id);
    Ok(())
}

/// 获取指定会话的最新监控快照
///
/// 委托给 session_manager，由其从 monitor_service 的快照缓存中读取数据。
/// 若该会话尚无监控数据，返回错误提示。
#[tauri::command]
pub fn get_monitor_status(
    session_id: String,
    session_manager: State<'_, Mutex<SessionManager>>,
) -> Result<MonitorSnapshot, String> {
    session_manager
        .lock()
        .map_err(|error| error.to_string())?
        .get_monitor_snapshot(&session_id)
        .ok_or_else(|| "未找到该会话的监控数据".to_string())
}
