use crate::commands::run_blocking_op;
use crate::core::host_service::SharedHostConfigService;
use crate::errors::app_error::AppErrorInfo;
use crate::models::host::HostConfig;
use crate::models::host::SaveHostRequest;
use tauri::{AppHandle, Manager};

/// 列出所有已保存的主机配置,不含明文凭据
///
/// 薄 adapter:从受管共享服务持锁读取并委托,业务规则在 host_service。
/// 持锁保证读不到写了一半的 hosts.json;spawn_blocking 保证不占用主线程。
#[tauri::command]
pub async fn list_hosts(app: AppHandle) -> Result<Vec<HostConfig>, AppErrorInfo> {
    run_blocking_op(move || {
        app.state::<SharedHostConfigService>()
            .with_locked(|service| service.list_hosts())
    })
    .await
}

/// 保存主机配置：将明文凭据写入 OS 安全存储，仅将引用键落盘；
/// endpoint 变更且旧值不再被任何配置引用时，自动清理旧信任记录
///
/// 薄 adapter：校验、凭据写入、引用解析、落盘、失败补偿与信任清理全部委托
/// host_service。与 list/delete 共用同一受管服务实例，整个 load-modify-write
/// 周期持锁串行化，并发 invoke 不会互相覆盖。
///
/// # 参数
/// - `request`: 含明文凭据的保存请求，处理完毕后明文不得持久化
///
/// # 返回
/// 更新后的主机列表
#[tauri::command]
pub async fn save_host(
    app: AppHandle,
    request: SaveHostRequest,
) -> Result<Vec<HostConfig>, AppErrorInfo> {
    run_blocking_op(move || {
        app.state::<SharedHostConfigService>()
            .with_locked(|service| service.save(&request))
    })
    .await
}

/// 删除主机配置：同步清理 OS 安全存储凭据，并在被删 endpoint 不再被
/// 任何剩余配置引用时清理其信任记录
///
/// 与 list/save 共用同一受管服务实例，整个 load-modify-write 周期持锁
/// 串行化，并发 invoke 不会互相覆盖。
///
/// # 参数
/// - `host_id`: 要删除的主机 ID
///
/// # 返回
/// 更新后的主机列表
#[tauri::command]
pub async fn delete_host(app: AppHandle, host_id: String) -> Result<Vec<HostConfig>, AppErrorInfo> {
    run_blocking_op(move || {
        app.state::<SharedHostConfigService>()
            .with_locked(|service| service.delete(&host_id))
    })
    .await
}
