use crate::commands::run_blocking_op;
use crate::core::sftp_service::SftpService;
use crate::errors::app_error::AppErrorInfo;
use crate::models::sftp::{ConflictStrategy, RemoteEntry, TransferTask};
use tauri::{AppHandle, Manager, Runtime, State};

/// 列举远程目录内容，按目录优先、名称排序
///
/// 异步 command：会等待控制连接就绪（TCP/SSH 握手、host-identity challenge 未决），
/// 等待发生在阻塞线程池，不得占用 Tauri 主线程（否则前端整体卡死）。
///
/// # 参数
/// - `session_id`: 关联的 SSH 会话 ID
/// - `path`: 远程目录绝对路径
#[tauri::command]
pub async fn sftp_list_dir<R: Runtime>(
    session_id: String,
    path: String,
    app: AppHandle<R>,
) -> Result<Vec<RemoteEntry>, AppErrorInfo> {
    let service = app.state::<SftpService>().inner().clone();
    run_blocking_op(move || service.list_dir(&session_id, &path)).await
}

/// 发起文件下载任务，立即返回 status = Pending 的 TransferTask
///
/// 异步 command：入队前需在控制连接上查询远端文件大小，等待发生在阻塞线程池。
///
/// # 参数
/// - `session_id`: 关联的 SSH 会话 ID
/// - `remote_path`: 远程文件完整路径
/// - `local_path`: 本地保存路径（父目录必须存在）
/// - `conflict_strategy`: 目标已存在时的处理策略，缺省 Reject（拒绝覆盖）
#[tauri::command]
pub async fn sftp_download<R: Runtime>(
    app: AppHandle<R>,
    session_id: String,
    remote_path: String,
    local_path: String,
    conflict_strategy: Option<ConflictStrategy>,
) -> Result<TransferTask, AppErrorInfo> {
    let service = app.state::<SftpService>().inner().clone();
    run_blocking_op(move || {
        service.enqueue_download(
            session_id,
            remote_path,
            local_path,
            conflict_strategy.unwrap_or_default(),
            app,
        )
    })
    .await
}

/// 发起文件上传任务，立即返回 status = Pending 的 TransferTask
///
/// # 参数
/// - `session_id`: 关联的 SSH 会话 ID
/// - `local_path`: 本地文件完整路径
/// - `remote_path`: 远程目标目录路径（后端自动拼接文件名）
/// - `conflict_strategy`: 目标已存在时的处理策略，缺省 Reject（拒绝覆盖）
#[tauri::command]
pub fn sftp_upload<R: Runtime>(
    app: AppHandle<R>,
    session_id: String,
    local_path: String,
    remote_path: String,
    conflict_strategy: Option<ConflictStrategy>,
    sftp_service: State<'_, SftpService>,
) -> Result<TransferTask, AppErrorInfo> {
    sftp_service
        .enqueue_upload(
            session_id,
            local_path,
            remote_path,
            conflict_strategy.unwrap_or_default(),
            app,
        )
        .map_err(AppErrorInfo::from)
}

/// 取消指定传输任务；任务不存在时拒绝并返回结构化错误，已终态任务静默成功
///
/// # 参数
/// - `task_id`: 要取消的任务 ID（全局唯一 UUID）
#[tauri::command]
pub fn sftp_cancel_task(
    task_id: String,
    sftp_service: State<'_, SftpService>,
) -> Result<(), AppErrorInfo> {
    sftp_service
        .cancel_task(&task_id)
        .map_err(AppErrorInfo::from)
}

/// 获取指定 Session 的权威任务快照（按 createdAt 最新优先），供前端恢复错过的事件
///
/// # 参数
/// - `session_id`: 关联的 SSH 会话 ID
#[tauri::command]
pub fn sftp_task_snapshot(
    session_id: String,
    sftp_service: State<'_, SftpService>,
) -> Vec<TransferTask> {
    sftp_service.task_snapshot(&session_id)
}

/// 清除指定 Session 的全部终态任务记录；Pending/Running 活动任务不受影响
///
/// 幂等：无终态任务或 Session 不存在时静默成功。
///
/// # 参数
/// - `session_id`: 关联的 SSH 会话 ID
#[tauri::command]
pub fn sftp_clear_terminal_tasks(session_id: String, sftp_service: State<'_, SftpService>) {
    sftp_service.clear_terminal_tasks(&session_id);
}

#[cfg(test)]
#[path = "sftp_test.rs"]
mod tests;
