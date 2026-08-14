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
mod tests {
    use super::{sftp_download, sftp_list_dir, sftp_upload};
    use crate::core::sftp_service::SftpService;
    use crate::core::ssh_transport::test_support::memory_sftp;
    use crate::models::host::{AuthType, HostConfig};
    use crate::models::sftp::{SftpTaskStatus, TransferTask};
    use std::sync::mpsc;
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

    /// 回归（Linux 双击连接界面卡死）：sftp_list_dir 会等待控制连接就绪
    /// （TCP 握手 / SSH 握手 / 主机身份 challenge 未决）。invoke 调度（on_message）
    /// 必须立即返回：真实应用中同步 command 在 Tauri 主线程内联执行命令体，
    /// 主线程被 Condvar 等待占用时前端所有交互（含 challenge 确认点击）被阻塞，
    /// 界面整体卡死。
    #[test]
    fn sftp_list_dir_dispatch_does_not_block_invoke_caller() {
        let (release_tx, release_rx) = mpsc::channel::<()>();
        // connector 闭包要求 Sync：Receiver 包裹在 Mutex 中（连接器仅调用一次）
        let release_rx = std::sync::Mutex::new(release_rx);
        let service = SftpService::with_connector(move |_host, _role| {
            release_rx
                .lock()
                .expect("release rx 锁应可用")
                .recv()
                .expect("release 信号应送达");
            Ok(memory_sftp(vec![]))
        });
        let managed = service.clone();
        service.register_session("session-1".to_string(), make_host());

        let app = mock_builder()
            .manage(managed)
            .invoke_handler(tauri::generate_handler![sftp_list_dir])
            .build(mock_context(noop_assets()))
            .unwrap();
        let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .unwrap();

        // 在独立线程调用 invoke，模拟 Tauri 主线程
        let (dispatched_tx, dispatched_rx) = mpsc::channel::<()>();
        let dispatch = std::thread::spawn({
            let webview = webview.clone();
            move || {
                webview.on_message(
                    request(
                        "sftp_list_dir",
                        serde_json::json!({ "sessionId": "session-1", "path": "/" }),
                    ),
                    Box::new(|_window, _cmd, _response, _callback, _error| {}),
                );
                dispatched_tx.send(()).expect("dispatch 完成信号应送达");
            }
        });

        // 控制连接未决时 invoke 调度必须立即返回；同步 command 会在调用线程
        // 内联等待连接就绪，释放前无法返回。
        dispatched_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("invoke 调度必须立即返回（同步 command 会阻塞调用线程直到连接就绪）");

        // 解除阻塞后命令在后台正常完成
        release_tx.send(()).expect("release 信号应送达");
        dispatch.join().expect("调用线程应正常返回");
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

    /// 缺省 conflictStrategy 时上传按 Reject 处理：远端目标已存在的上传最终 Failed，
    /// 结构化错误为 SftpTargetExists，远端旧内容保持不动。
    #[test]
    fn sftp_upload_without_conflict_strategy_defaults_to_reject() {
        use crate::core::ssh_transport::test_support::{in_memory_sftp, in_memory_sftp_transport};

        let fs = in_memory_sftp(&[("/srv/keep.txt", b"old".to_vec())]);
        let fs_for_connector = fs.clone();
        let service = SftpService::with_connector(move |_, _| {
            Ok(in_memory_sftp_transport(&fs_for_connector))
        });
        let managed = service.clone();
        service.register_session("session-1".to_string(), make_host());

        let app = mock_builder()
            .manage(managed)
            .invoke_handler(tauri::generate_handler![sftp_upload])
            .build(mock_context(noop_assets()))
            .unwrap();
        let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .unwrap();

        // 独立子目录承载同名本地文件：远端目标 /srv/keep.txt 与预置目标冲突
        let local_dir =
            std::env::temp_dir().join(format!("titan-cmd-upload-default-{}", Uuid::new_v4()));
        std::fs::create_dir(&local_dir).unwrap();
        let local_path = local_dir.join("keep.txt");
        std::fs::write(&local_path, b"new").unwrap();

        let response = get_ipc_response(
            &webview,
            request(
                "sftp_upload",
                serde_json::json!({
                    "sessionId": "session-1",
                    "localPath": local_path.to_string_lossy(),
                    "remotePath": "/srv",
                }),
            ),
        )
        .expect("缺省 conflictStrategy 的上传命令应成功入队");
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
            fs.content("/srv/keep.txt"),
            Some(b"old".to_vec()),
            "缺省 Reject 不得覆盖远端旧文件"
        );
        let _ = std::fs::remove_dir_all(&local_dir);
    }

    /// 显式 Overwrite 策略经上传命令透传：远端目标被原子替换，任务 Done。
    #[test]
    fn sftp_upload_with_explicit_overwrite_replaces_remote_target() {
        use crate::core::ssh_transport::test_support::{in_memory_sftp, in_memory_sftp_transport};

        let fs = in_memory_sftp(&[("/srv/keep.txt", b"old".to_vec())]);
        let fs_for_connector = fs.clone();
        let service = SftpService::with_connector(move |_, _| {
            Ok(in_memory_sftp_transport(&fs_for_connector))
        });
        let managed = service.clone();
        service.register_session("session-1".to_string(), make_host());

        let app = mock_builder()
            .manage(managed)
            .invoke_handler(tauri::generate_handler![sftp_upload])
            .build(mock_context(noop_assets()))
            .unwrap();
        let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .unwrap();

        let local_dir =
            std::env::temp_dir().join(format!("titan-cmd-upload-overwrite-{}", Uuid::new_v4()));
        std::fs::create_dir(&local_dir).unwrap();
        let local_path = local_dir.join("keep.txt");
        std::fs::write(&local_path, b"new").unwrap();

        let response = get_ipc_response(
            &webview,
            request(
                "sftp_upload",
                serde_json::json!({
                    "sessionId": "session-1",
                    "localPath": local_path.to_string_lossy(),
                    "remotePath": "/srv",
                    "conflictStrategy": "Overwrite",
                }),
            ),
        )
        .expect("显式 Overwrite 的上传命令应成功入队");
        let task: TransferTask = response.deserialize().unwrap();
        assert_eq!(task.status, SftpTaskStatus::Pending);

        let terminal = wait_terminal(&service, "session-1", &task.task_id);
        assert_eq!(terminal.status, SftpTaskStatus::Done);
        assert_eq!(
            fs.content("/srv/keep.txt"),
            Some(b"new".to_vec()),
            "确认覆盖后远端目标内容应为新内容"
        );
        let _ = std::fs::remove_dir_all(&local_dir);
    }
}
