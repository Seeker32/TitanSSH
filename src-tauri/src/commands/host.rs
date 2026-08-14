use crate::core::host_service::HostConfigService;
use crate::errors::app_error::AppErrorInfo;
use crate::models::host::HostConfig;
use crate::models::host::SaveHostRequest;
use tauri::AppHandle;

/// 列出所有已保存的主机配置,不含明文凭据
///
/// 薄 adapter:只负责构造 HostConfigService 并委托,业务规则在 host_service
#[tauri::command]
pub fn list_hosts(app: AppHandle) -> Result<Vec<HostConfig>, AppErrorInfo> {
    let service = HostConfigService::new(&app)?;
    service.list_hosts().map_err(AppErrorInfo::from)
}

/// 保存主机配置：将明文凭据写入 OS 安全存储，仅将引用键落盘；
/// endpoint 变更且旧值不再被任何配置引用时，自动清理旧信任记录
///
/// 薄 adapter：校验、凭据写入、引用解析、落盘、失败补偿与信任清理全部委托
/// host_service。
///
/// # 参数
/// - `request`: 含明文凭据的保存请求，处理完毕后明文不得持久化
///
/// # 返回
/// 更新后的主机列表
#[tauri::command]
pub fn save_host(
    app: AppHandle,
    request: SaveHostRequest,
) -> Result<Vec<HostConfig>, AppErrorInfo> {
    let service = HostConfigService::new(&app)?;
    service.save(&request).map_err(AppErrorInfo::from)
}

/// 删除主机配置：同步清理 OS 安全存储凭据，并在被删 endpoint 不再被
/// 任何剩余配置引用时清理其信任记录
///
/// # 参数
/// - `host_id`: 要删除的主机 ID
///
/// # 返回
/// 更新后的主机列表
#[tauri::command]
pub fn delete_host(app: AppHandle, host_id: String) -> Result<Vec<HostConfig>, AppErrorInfo> {
    let service = HostConfigService::new(&app)?;
    service.delete(&host_id).map_err(AppErrorInfo::from)
}
