use crate::errors::app_error::AppError;
use crate::models::host::HostConfig;
use std::fs;
use std::path::PathBuf;
use tauri::{AppHandle, Manager};

pub struct HostStore {
    file_path: PathBuf,
}

impl HostStore {
    /// Creates a new HostStore instance
    /// 
    /// # Arguments
    /// * `app_handle` - Tauri application handle for accessing app data directory
    /// 
    /// # Returns
    /// * `Result<Self, AppError>` - New HostStore instance or error
    pub fn new(app_handle: &AppHandle) -> Result<Self, AppError> {
        let app_data_dir = app_handle.path().app_data_dir().map_err(|error| {
            AppError::StorageError(format!("Failed to get app data dir: {error}"))
        })?;

        // Create directory if it doesn't exist
        fs::create_dir_all(&app_data_dir).map_err(|error| {
            AppError::StorageError(format!("Failed to create app data directory: {error}"))
        })?;

        let file_path = app_data_dir.join("hosts.json");

        Ok(Self { file_path })
    }

    #[cfg(test)]
    fn from_file_path(file_path: PathBuf) -> Self {
        Self { file_path }
    }

    /// Loads all host configurations from storage
    /// 
    /// # Returns
    /// * `Result<Vec<HostConfig>, AppError>` - List of host configs or error
    pub fn load(&self) -> Result<Vec<HostConfig>, AppError> {
        // Return empty list if file doesn't exist
        if !self.file_path.exists() {
            return Ok(Vec::new());
        }

        let content = fs::read_to_string(&self.file_path).map_err(|error| {
            AppError::StorageError(format!("Failed to read hosts file: {error}"))
        })?;

        let hosts: Vec<HostConfig> = serde_json::from_str(&content).map_err(|error| {
            AppError::StorageError(format!("Failed to parse hosts file: {error}"))
        })?;

        Ok(hosts)
    }

    /// Saves host configurations to storage
    /// 
    /// # Arguments
    /// * `hosts` - Slice of host configurations to save
    /// 
    /// # Returns
    /// * `Result<(), AppError>` - Success or error
    pub fn save(&self, hosts: &[HostConfig]) -> Result<(), AppError> {
        let content = serde_json::to_string_pretty(hosts).map_err(|error| {
            AppError::StorageError(format!("Failed to serialize hosts: {error}"))
        })?;

        fs::write(&self.file_path, content).map_err(|error| {
            AppError::StorageError(format!("Failed to write hosts file: {error}"))
        })?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::HostStore;
    use crate::models::host::{AuthType, HostConfig};
    use std::fs;
    use std::path::PathBuf;
    use uuid::Uuid;

    fn temp_hosts_file() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("titan-host-store-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).expect("temp dir should be created");
        dir.join("hosts.json")
    }

    fn sample_host() -> HostConfig {
        HostConfig {
            id: "host-1".to_string(),
            name: "prod".to_string(),
            host: "10.0.0.8".to_string(),
            port: 22,
            username: "root".to_string(),
            auth_type: AuthType::Password,
            password: Some("secret".to_string()),
            private_key_path: None,
            passphrase: None,
            remark: Some("primary".to_string()),
        }
    }

    #[test]
    fn load_returns_empty_when_file_does_not_exist() {
        let store = HostStore::from_file_path(temp_hosts_file());
        let hosts = store.load().expect("load should succeed");
        assert!(hosts.is_empty());
    }

    #[test]
    fn save_and_load_round_trip_hosts() {
        let store = HostStore::from_file_path(temp_hosts_file());
        let hosts = vec![sample_host()];

        store.save(&hosts).expect("save should succeed");
        let loaded = store.load().expect("load should succeed");

        assert_eq!(loaded, hosts);
    }

    #[test]
    fn load_returns_error_for_invalid_json() {
        let file_path = temp_hosts_file();
        fs::write(&file_path, "{not-json").expect("invalid json should be written");
        let store = HostStore::from_file_path(file_path);

        let error = store.load().expect_err("load should fail");
        assert!(error.to_string().contains("Failed to parse hosts file"));
    }
}
