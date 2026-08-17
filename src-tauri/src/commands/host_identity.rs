use crate::commands::run_blocking_op;
use crate::core::session_manager::SessionManager;
use crate::errors::app_error::AppErrorInfo;
use crate::models::host_identity::TrustedHostInfo;
use tauri::{AppHandle, State};

/// 列出 Settings“可信主机”只读清单
///
/// 薄 adapter：从 HostIdentityService 读取全部持久化信任记录（按 host 字典序 +
/// port 稳定排序），返回 endpoint、算法与 SHA-256 指纹的 typed JSON。
/// 信任存储读取/解析失败时以 TrustStoreError 结构化返回，绝不伪装成空列表；
/// 前端只消费本结果，不解析 known_hosts 文本。
///
/// 异步 command：known_hosts 的完整读取与解析可能被慢速文件系统阻塞，必须在线程池
/// 执行，避免占用 Tauri 主线程。
#[tauri::command]
pub async fn list_trusted_hosts(
    session_manager: State<'_, SessionManager>,
) -> Result<Vec<TrustedHostInfo>, AppErrorInfo> {
    let identity_service = session_manager.identity_service().clone();
    run_blocking_op(move || identity_service.list_trusted_hosts()).await
}

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

/// 接受并保存未知主机身份
///
/// 把 challenge 快照的算法与完整公钥持久化到 TitanSSH 独立 known_hosts 文件，
/// 随后与 accept 一致：为该 Runtime Session 记录临时信任并唤醒全部等待连接。
/// 保存失败时 challenge 保持未决并以 HostKeySaveFailed 结构化返回，
/// 前端保持确认卡并展示错误，可重试保存、改选仅本次接受或拒绝。
///
/// 异步 command：known_hosts 的安全发布包含阻塞文件 I/O，并会在保存期间持有
/// host-identity 状态锁；该过程必须在线程池执行，不能阻塞命令分发线程。
#[tauri::command]
pub async fn accept_and_save_host_identity(
    app: AppHandle,
    challenge_id: String,
    session_manager: State<'_, SessionManager>,
) -> Result<(), AppErrorInfo> {
    let identity_service = session_manager.identity_service().clone();
    run_blocking_op(move || identity_service.accept_and_save(&app, &challenge_id)).await
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
