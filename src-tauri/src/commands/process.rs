use crate::commands::run_blocking_op;
use crate::core::process_service::ProcessService;
use crate::core::session_manager::SessionManager;
use crate::errors::app_error::{AppError, AppErrorInfo};
use crate::models::monitor::TaskInfo;
use crate::models::process::ProcessSnapshot;
use tauri::{AppHandle, Manager, Runtime, State};

/// 为指定会话启动进程采样任务。
#[tauri::command]
pub async fn start_process_monitoring<R: Runtime>(
    app: AppHandle<R>,
    session_id: String,
    session_manager: State<'_, SessionManager>,
    process_service: State<'_, ProcessService>,
) -> Result<TaskInfo, AppErrorInfo> {
    let host = session_manager
        .host_config(&session_id)
        .map_err(AppErrorInfo::from)?;
    let verifier = session_manager
        .host_key_verifier(&app, &session_id)
        .map_err(AppErrorInfo::from)?;
    let service = process_service.inner().clone();
    run_blocking_op(move || service.start_process_monitoring(session_id, host, verifier, app)).await
}

/// 停止指定进程采样任务；未知任务返回稳定的 ProcessTaskNotFound。
#[tauri::command]
pub async fn stop_process_monitoring<R: Runtime>(
    app: AppHandle<R>,
    task_id: String,
    process_service: State<'_, ProcessService>,
) -> Result<(), AppErrorInfo> {
    let service = process_service.inner().clone();
    run_blocking_op(move || {
        if service.stop_process_monitoring(&app, &task_id) {
            Ok(())
        } else {
            Err(AppError::ProcessTaskNotFound(task_id.into()))
        }
    })
    .await
}

/// 获取指定会话缓存的最新进程快照。
#[tauri::command]
pub async fn get_process_status<R: Runtime>(
    app: AppHandle<R>,
    session_id: String,
    process_service: State<'_, ProcessService>,
) -> Result<ProcessSnapshot, AppErrorInfo> {
    let service = process_service.inner().clone();
    run_blocking_op(move || {
        app.state::<SessionManager>().host_config(&session_id)?;
        service
            .get_process_status(&session_id)
            .ok_or_else(|| AppError::ProcessSnapshotUnavailable(session_id.into()))
    })
    .await
}

#[cfg(test)]
#[path = "process_test.rs"]
mod tests;
