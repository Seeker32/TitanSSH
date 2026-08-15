use crate::errors::app_error::AppError;
use crate::models::host::HostConfig;
use std::fs;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager};

const LEGACY_IDENTIFIER: &str = "dev.titanssh.ssh-terminal-manager";
const HOSTS_FILE_NAME: &str = "hosts.json";

/// 将开发期 identifier 目录中的主机配置复制到正式目录
fn migrate_legacy_hosts(legacy_file: &Path, new_file: &Path) -> Result<(), AppError> {
    if new_file.exists() || !legacy_file.exists() {
        return Ok(());
    }

    fs::copy(legacy_file, new_file)
        .map(|_| ())
        .map_err(|error| AppError::StorageError(format!("迁移旧主机配置失败: {error}")))?;
    Ok(())
}

pub struct HostStore {
    file_path: PathBuf,
}

impl HostStore {
    /// 创建新的 HostStore 实例
    ///
    /// 通过 Tauri AppHandle 获取应用数据目录，确保目录存在后构建 hosts.json 文件路径。
    ///
    /// # 参数
    /// - `app_handle`: Tauri 应用句柄，用于解析平台相关的应用数据目录
    ///
    /// # 返回
    /// 成功返回 HostStore 实例，失败返回 StorageError
    pub fn new(app_handle: &AppHandle) -> Result<Self, AppError> {
        let app_data_dir = app_handle
            .path()
            .app_data_dir()
            .map_err(|error| AppError::StorageError(format!("无法获取应用数据目录: {error}")))?;

        // 确保数据目录存在，首次运行时自动创建
        fs::create_dir_all(&app_data_dir)
            .map_err(|error| AppError::StorageError(format!("无法创建应用数据目录: {error}")))?;

        let file_path = app_data_dir.join(HOSTS_FILE_NAME);
        if let Some(data_root) = app_data_dir.parent() {
            let legacy_file = data_root.join(LEGACY_IDENTIFIER).join(HOSTS_FILE_NAME);
            migrate_legacy_hosts(&legacy_file, &file_path)?;
        }

        Ok(Self { file_path })
    }

    /// 仅供测试使用：直接通过文件路径构造 HostStore，绕过 AppHandle
    #[cfg(test)]
    pub(crate) fn from_file_path(file_path: PathBuf) -> Self {
        Self { file_path }
    }

    /// 从持久化存储加载所有主机配置
    ///
    /// 若 hosts.json 不存在则返回空列表（首次运行场景）。
    /// 文件存在但内容非法时返回 StorageError。
    ///
    /// # 返回
    /// 成功返回主机配置列表，失败返回 StorageError
    pub fn load(&self) -> Result<Vec<HostConfig>, AppError> {
        // 文件不存在时返回空列表，对应首次运行场景
        if !self.file_path.exists() {
            return Ok(Vec::new());
        }

        let content = fs::read_to_string(&self.file_path)
            .map_err(|error| AppError::StorageError(format!("读取主机配置文件失败: {error}")))?;

        let hosts: Vec<HostConfig> = serde_json::from_str(&content)
            .map_err(|error| AppError::StorageError(format!("解析主机配置文件失败: {error}")))?;

        Ok(hosts)
    }

    /// 将主机配置列表持久化到 hosts.json
    ///
    /// 使用 pretty-print JSON 格式写入，便于人工排查问题。
    /// 写入前不含任何明文凭据，调用方必须确保已完成凭据剥离。
    ///
    /// # 参数
    /// - `hosts`: 要持久化的主机配置切片（不含明文凭据）
    pub fn save(&self, hosts: &[HostConfig]) -> Result<(), AppError> {
        let content = serde_json::to_string_pretty(hosts)
            .map_err(|error| AppError::StorageError(format!("序列化主机配置失败: {error}")))?;

        fs::write(&self.file_path, content)
            .map_err(|error| AppError::StorageError(format!("写入主机配置文件失败: {error}")))?;

        Ok(())
    }
}

#[cfg(test)]
#[path = "host_store_test.rs"]
mod tests;
