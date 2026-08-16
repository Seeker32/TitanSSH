use crate::commands::run_blocking_op;
use crate::core::monitor_service::MonitorService;
use crate::core::session_manager::SessionManager;
use crate::errors::app_error::{AppError, AppErrorInfo};
use crate::models::monitor::{MonitorSnapshot, TaskInfo};
use tauri::{AppHandle, Runtime, State};

/// 为指定会话启动监控任务
///
/// 从 session_manager 读取主机配置与主机身份统一校验器后，
/// 委托 monitor_service 读取凭据并创建后台采集任务；监控连接与其他 capability
/// 一样在握手后、认证前经过统一校验。
/// 返回包含 task_id 的 TaskInfo，前端可用于跟踪任务状态。
/// 凭据读取失败或 session 不存在时返回错误字符串。
///
/// 异步 command：凭据读取会访问 OS 安全存储（Linux DBus/keyring 可能等待
/// 授权或守护响应），必须在阻塞线程池执行，不得占用 Tauri 主线程（否则前端卡死）。
#[tauri::command]
pub async fn start_monitoring<R: Runtime>(
    app: AppHandle<R>,
    session_id: String,
    session_manager: State<'_, SessionManager>,
    monitor_service: State<'_, MonitorService>,
) -> Result<TaskInfo, AppErrorInfo> {
    let host = session_manager
        .host_config(&session_id)
        .map_err(AppErrorInfo::from)?;
    let verifier = session_manager
        .host_key_verifier(&app, &session_id)
        .map_err(AppErrorInfo::from)?;
    let service = monitor_service.inner().clone();
    run_blocking_op(move || service.start_monitoring(session_id, host, verifier, app)).await
}

/// 停止指定 task_id 对应的监控任务
///
/// 委托给 monitor_service 设置关闭标志并清理任务句柄；终态 Done 事件由
/// monitor_service 直接补发（worker 移除后不再能广播）。
/// 任务不存在（从未创建、已停止或已过期）时返回结构化错误
/// MonitorTaskNotFound，前端可据此区分「已停止」与「早已消失」，
/// 暴露陈旧/重复的任务状态。
#[tauri::command]
pub fn stop_monitoring<R: Runtime>(
    app: AppHandle<R>,
    task_id: String,
    monitor_service: State<'_, MonitorService>,
) -> Result<(), AppErrorInfo> {
    if monitor_service.stop_monitoring(&app, &task_id) {
        Ok(())
    } else {
        Err(AppErrorInfo::from(AppError::MonitorTaskNotFound(
            task_id.into(),
        )))
    }
}

/// 获取指定会话的最新监控快照
///
/// 先从 session_manager 做会话存在性权威判定：仅当会话确实不存在时返回
/// SessionNotFound；会话存在但尚无快照（首轮采集完成前、或监控已停止/失败）
/// 返回 MonitorSnapshotUnavailable。SessionNotFound 是 close_session 式
/// teardown 的键，瞬时无数据不得伪装成「会话已消失」触发前端拆除会话状态。
#[tauri::command]
pub fn get_monitor_status(
    session_id: String,
    session_manager: State<'_, SessionManager>,
    monitor_service: State<'_, MonitorService>,
) -> Result<MonitorSnapshot, AppErrorInfo> {
    session_manager
        .host_config(&session_id)
        .map_err(AppErrorInfo::from)?;
    monitor_service
        .get_monitor_status(&session_id)
        .ok_or_else(|| AppErrorInfo::from(AppError::MonitorSnapshotUnavailable(session_id.into())))
}

#[cfg(test)]
#[path = "monitor_test.rs"]
mod tests;
