use crate::core::session_manager::SessionManager;
use crate::errors::app_error::AppErrorInfo;
use tauri::{AppHandle, State};

/// 仅本次接受未知主机身份
///
/// 为该 Runtime Session 记录临时信任（覆盖 Terminal、SFTP、Monitoring 及重连），
/// 并唤醒同一 challenge 上的全部等待连接继续认证。
/// challenge 不存在（已解决或从未创建）时返回稳定错误。
#[tauri::command]
pub fn accept_host_identity(
    challenge_id: String,
    session_manager: State<'_, SessionManager>,
) -> Result<(), AppErrorInfo> {
    session_manager
        .identity_service()
        .accept(&challenge_id)
        .map_err(AppErrorInfo::from)
}

/// 拒绝未知主机身份并关闭整个 Session
///
/// 同一 challenge 上的全部等待连接以 HostKeyRejected 失败（不进入认证），
/// 随后对所属 Runtime Session 执行后端 teardown：Terminal、SFTP 与 Monitoring
/// 服从同一决定。Session 已关闭时忽略重复 teardown。
#[tauri::command]
pub fn reject_host_identity(
    app: AppHandle,
    challenge_id: String,
    session_manager: State<'_, SessionManager>,
) -> Result<(), AppErrorInfo> {
    let challenge = session_manager
        .identity_service()
        .reject(&challenge_id)
        .map_err(AppErrorInfo::from)?;
    if let Err(error) = session_manager.close_session(&challenge.session_id, &app) {
        // Session 可能已被用户关闭（关闭标签等路径），重复 teardown 不视为错误
        log::info!(
            "[host-identity] reject teardown skipped for session {}: {:?}",
            challenge.session_id,
            error.code()
        );
    }
    Ok(())
}
