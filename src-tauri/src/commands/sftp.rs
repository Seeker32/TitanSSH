use crate::core::sftp_service::SftpService;
use crate::errors::app_error::AppErrorInfo;
use crate::models::sftp::{RemoteEntry, TransferTask};
use tauri::{AppHandle, State};

/// 列举远程目录内容，按目录优先、名称排序
///
/// # 参数
/// - `session_id`: 关联的 SSH 会话 ID
/// - `path`: 远程目录绝对路径
#[tauri::command]
pub fn sftp_list_dir(
    session_id: String,
    path: String,
    sftp_service: State<'_, SftpService>,
) -> Result<Vec<RemoteEntry>, AppErrorInfo> {
    sftp_service
        .list_dir(&session_id, &path)
        .map_err(AppErrorInfo::from)
}

/// 发起文件下载任务，立即返回 status = Pending 的 TransferTask
///
/// # 参数
/// - `session_id`: 关联的 SSH 会话 ID
/// - `remote_path`: 远程文件完整路径
/// - `local_path`: 本地保存路径（父目录必须存在）
#[tauri::command]
pub fn sftp_download(
    app: AppHandle,
    session_id: String,
    remote_path: String,
    local_path: String,
    sftp_service: State<'_, SftpService>,
) -> Result<TransferTask, AppErrorInfo> {
    sftp_service
        .enqueue_download(session_id, remote_path, local_path, app)
        .map_err(AppErrorInfo::from)
}

/// 发起文件上传任务，立即返回 status = Pending 的 TransferTask
///
/// # 参数
/// - `session_id`: 关联的 SSH 会话 ID
/// - `local_path`: 本地文件完整路径
/// - `remote_path`: 远程目标目录路径（后端自动拼接文件名）
#[tauri::command]
pub fn sftp_upload(
    app: AppHandle,
    session_id: String,
    local_path: String,
    remote_path: String,
    sftp_service: State<'_, SftpService>,
) -> Result<TransferTask, AppErrorInfo> {
    sftp_service
        .enqueue_upload(session_id, local_path, remote_path, app)
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
