use crate::core::sftp_service::SftpService;
use crate::errors::app_error::AppErrorInfo;
use crate::models::sftp::{DownloadConflictStrategy, RemoteEntry, TransferTask};
use tauri::{AppHandle, Runtime, State};

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
/// - `conflict_strategy`: 目标已存在时的处理策略，缺省 Reject（拒绝覆盖）
#[tauri::command]
pub fn sftp_download<R: Runtime>(
    app: AppHandle<R>,
    session_id: String,
    remote_path: String,
    local_path: String,
    conflict_strategy: Option<DownloadConflictStrategy>,
    sftp_service: State<'_, SftpService>,
) -> Result<TransferTask, AppErrorInfo> {
    sftp_service
        .enqueue_download(
            session_id,
            remote_path,
            local_path,
            conflict_strategy.unwrap_or_default(),
            app,
        )
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
mod tests {
    use super::sftp_download;
    use crate::core::sftp_service::SftpService;
    use crate::core::ssh_transport::test_support::memory_sftp;
    use crate::models::host::{AuthType, HostConfig};
    use crate::models::sftp::{SftpTaskStatus, TransferTask};
    use std::time::Duration;
    use tauri::ipc::{CallbackFn, InvokeBody};
    use tauri::test::{INVOKE_KEY, get_ipc_response, mock_builder, mock_context, noop_assets};
    use tauri::webview::InvokeRequest;
    use uuid::Uuid;

    /// 构造不含明文凭据的测试主机。
    fn make_host() -> HostConfig {
        HostConfig {
            id: "host-1".to_string(),
            name: "test".to_string(),
            host: "127.0.0.1".to_string(),
            port: 22,
            username: "root".to_string(),
            auth_type: AuthType::Password,
            password_ref: Some("ref".to_string()),
            private_key_path: None,
            passphrase_ref: None,
            remark: None,
            group: String::new(),
        }
    }

    /// 构造同步 IPC 请求；参数走 camelCase 键（与前端 invoke 一致）。
    fn request(cmd: &str, body: serde_json::Value) -> InvokeRequest {
        InvokeRequest {
            cmd: cmd.to_string(),
            callback: CallbackFn(0),
            error: CallbackFn(1),
            url: "http://tauri.localhost".parse().unwrap(),
            body: InvokeBody::Json(body),
            headers: Default::default(),
            invoke_key: INVOKE_KEY.to_string(),
        }
    }

    /// 轮询任务快照直到终态（最多 2 秒）。
    fn wait_terminal(service: &SftpService, session_id: &str, task_id: &str) -> TransferTask {
        for _ in 0..200 {
            let task = service
                .task_snapshot(session_id)
                .into_iter()
                .find(|task| task.task_id == task_id);
            if let Some(task) = task {
                if matches!(
                    task.status,
                    SftpTaskStatus::Done | SftpTaskStatus::Failed | SftpTaskStatus::Cancelled
                ) {
                    return task;
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("任务 {} 未在 2 秒内到达终态", task_id);
    }

    /// 缺省 conflictStrategy 时按 Reject 处理：目标已存在的下载最终 Failed，
    /// 结构化错误为 SftpTargetExists，原文件保持不动。
    #[test]
    fn sftp_download_without_conflict_strategy_defaults_to_reject() {
        let service = SftpService::with_connector(|_, _| Ok(memory_sftp(vec![1u8; 4])));
        let managed = service.clone();
        service.register_session("session-1".to_string(), make_host());

        let app = mock_builder()
            .manage(managed)
            .invoke_handler(tauri::generate_handler![sftp_download])
            .build(mock_context(noop_assets()))
            .unwrap();
        let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .unwrap();

        let local_path =
            std::env::temp_dir().join(format!("titan-cmd-default-{}.bin", Uuid::new_v4()));
        std::fs::write(&local_path, b"original").unwrap();

        let response = get_ipc_response(
            &webview,
            request(
                "sftp_download",
                serde_json::json!({
                    "sessionId": "session-1",
                    "remotePath": "/remote/file.bin",
                    "localPath": local_path.to_string_lossy(),
                }),
            ),
        )
        .expect("缺省 conflictStrategy 的下载命令应成功入队");
        let task: TransferTask = response.deserialize().unwrap();
        assert_eq!(task.status, SftpTaskStatus::Pending);

        let terminal = wait_terminal(&service, "session-1", &task.task_id);
        assert_eq!(terminal.status, SftpTaskStatus::Failed);
        assert_eq!(
            terminal.error.as_ref().map(|error| error.code.as_str()),
            Some("SftpTargetExists"),
            "缺省策略必须为 Reject，冲突时返回结构化 SftpTargetExists"
        );
        assert_eq!(
            std::fs::read(&local_path).unwrap(),
            b"original",
            "缺省 Reject 不得覆盖原有本地文件"
        );
        let _ = std::fs::remove_file(&local_path);
    }

    /// 显式 Overwrite 策略经命令透传：目标被原子替换，任务 Done。
    #[test]
    fn sftp_download_with_explicit_overwrite_replaces_target() {
        let service = SftpService::with_connector(|_, _| Ok(memory_sftp(vec![9u8; 8])));
        let managed = service.clone();
        service.register_session("session-1".to_string(), make_host());

        let app = mock_builder()
            .manage(managed)
            .invoke_handler(tauri::generate_handler![sftp_download])
            .build(mock_context(noop_assets()))
            .unwrap();
        let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .unwrap();

        let local_path =
            std::env::temp_dir().join(format!("titan-cmd-overwrite-{}.bin", Uuid::new_v4()));
        std::fs::write(&local_path, b"original").unwrap();

        let response = get_ipc_response(
            &webview,
            request(
                "sftp_download",
                serde_json::json!({
                    "sessionId": "session-1",
                    "remotePath": "/remote/file.bin",
                    "localPath": local_path.to_string_lossy(),
                    "conflictStrategy": "Overwrite",
                }),
            ),
        )
        .expect("显式 Overwrite 的下载命令应成功入队");
        let task: TransferTask = response.deserialize().unwrap();
        assert_eq!(task.status, SftpTaskStatus::Pending);

        let terminal = wait_terminal(&service, "session-1", &task.task_id);
        assert_eq!(terminal.status, SftpTaskStatus::Done);
        assert_eq!(
            std::fs::read(&local_path).unwrap(),
            vec![9u8; 8],
            "确认覆盖后目标内容应为远程内容"
        );
        let _ = std::fs::remove_file(&local_path);
    }
}
