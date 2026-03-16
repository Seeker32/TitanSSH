use crate::errors::app_error::AppError;
use crate::models::host::HostConfig;
use crate::storage::host_store::HostStore;
use tauri::AppHandle;

#[tauri::command]
pub fn list_hosts(app: AppHandle) -> Result<Vec<HostConfig>, String> {
    let store = HostStore::new(&app)?;
    store.load().map_err(String::from)
}

#[tauri::command]
pub fn save_host(app: AppHandle, host_config: HostConfig) -> Result<Vec<HostConfig>, String> {
    validate_host_config(&host_config)?;

    let store = HostStore::new(&app)?;
    let mut hosts = store.load()?;

    if let Some(index) = hosts.iter().position(|item| item.id == host_config.id) {
        hosts[index] = host_config;
    } else {
        hosts.push(host_config);
    }

    store.save(&hosts)?;
    Ok(hosts)
}

#[tauri::command]
pub fn delete_host(app: AppHandle, host_id: String) -> Result<Vec<HostConfig>, String> {
    let store = HostStore::new(&app)?;
    let mut hosts = store.load()?;
    hosts.retain(|host| host.id != host_id);
    store.save(&hosts)?;
    Ok(hosts)
}

fn validate_host_config(host: &HostConfig) -> Result<(), String> {
    if host.name.trim().is_empty() {
        return Err(String::from(AppError::InvalidHostConfig(
            "Host name is required".to_string(),
        )));
    }
    if host.host.trim().is_empty() {
        return Err(String::from(AppError::InvalidHostConfig(
            "Host address is required".to_string(),
        )));
    }
    if host.username.trim().is_empty() {
        return Err(String::from(AppError::InvalidHostConfig(
            "Username is required".to_string(),
        )));
    }
    Ok(())
}
