use crate::commands::run_blocking_op;
use crate::core::host_service::SharedHostConfigService;
use crate::core::session_manager::SessionManager;
use crate::errors::app_error::{AppError, AppErrorInfo, ErrorDetail};
use crate::models::session::SessionInfo;
use tauri::ipc::{InvokeBody, Request};
use tauri::{AppHandle, Manager, State};

/// 终端原始输入通过请求头关联所属 Runtime Session，避免把会话标识混入字节流。
const TERMINAL_SESSION_ID_HEADER: &str = "x-titanssh-session-id";

/// 在阻塞线程池读取打开会话所需的主机配置。
///
/// hosts.json 的读取可能因磁盘延迟或大型配置文件而阻塞；此处确保该工作不占用
/// Tauri 主线程。调用方仍负责把已读取的配置交给 session_manager 创建 Runtime Session。
async fn run_host_lookup<T, F>(lookup: F) -> Result<T, AppErrorInfo>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, AppError> + Send + 'static,
{
    run_blocking_op(lookup).await
}

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
pub async fn open_session(
    app: AppHandle,
    host_id: String,
    session_manager: State<'_, SessionManager>,
) -> Result<SessionInfo, AppErrorInfo> {
    // 从受管共享服务持锁查询主机配置（与 save/delete 串行化，避免读到半写入文件）；
    // hosts.json 的同步读取在线程池完成，不得占用 Tauri 主线程。
    let app_for_lookup = app.clone();
    let lookup_host_id = host_id.clone();
    let host = run_host_lookup(move || {
        app_for_lookup
            .state::<SharedHostConfigService>()
            .with_locked(|service| service.get_host(&lookup_host_id))
    })
    .await?
    .ok_or_else(|| AppErrorInfo::from(AppError::HostNotFound(host_id.into())))?;

    // 路由到 session_manager 协调层，由其启动 terminal_service
    session_manager
        .open_session(app, host)
        .map_err(AppErrorInfo::from)
}

#[cfg(test)]
#[path = "session_test.rs"]
mod tests;

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

/// 向指定会话的终端写入原始字节
///
/// 请求体使用 Tauri raw IPC payload，session id 通过固定请求头传递；
/// 字节不经过 UTF-8 解码，直接路由到 terminal_service 工作线程。
#[tauri::command]
pub fn write_terminal(
    request: Request<'_>,
    session_manager: State<'_, SessionManager>,
) -> Result<(), AppErrorInfo> {
    let session_id = request
        .headers()
        .get(TERMINAL_SESSION_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            AppErrorInfo::from(AppError::InvalidTerminalInput(ErrorDetail::msg(
                "终端输入请求缺少会话标识",
                Vec::new(),
            )))
        })?;
    let data = match request.body() {
        InvokeBody::Raw(data) => data.clone(),
        InvokeBody::Json(_) => {
            return Err(AppErrorInfo::from(AppError::InvalidTerminalInput(
                ErrorDetail::msg("终端输入请求必须使用原始字节 payload", Vec::new()),
            )));
        }
    };

    session_manager
        .write_terminal(session_id, data)
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
