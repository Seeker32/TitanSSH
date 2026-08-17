use crate::errors::app_error::{AppError, ErrorDetail};
use crate::models::host::HostConfig;
use log::{debug, info, warn};
use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager};
use uuid::Uuid;

const LEGACY_IDENTIFIER: &str = "dev.titanssh.ssh-terminal-manager";
const HOSTS_FILE_NAME: &str = "hosts.json";

/// 为损坏的主机配置生成同目录且唯一的隔离备份路径。
fn corrupt_hosts_backup_path(file_path: &Path) -> PathBuf {
    let file_name = file_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(HOSTS_FILE_NAME);
    file_path.with_file_name(format!("{file_name}.corrupt-{}", Uuid::new_v4()))
}

/// 将开发期 identifier 目录中的主机配置复制到正式目录。
///
/// 使用 `create_new` 原子地声明正式 hosts.json，避免先检查文件存在性再写入的
/// TOCTOU 竞争；文件已存在或旧文件不存在均为正常跳过。
fn migrate_legacy_hosts(legacy_file: &Path, new_file: &Path) -> Result<(), AppError> {
    let legacy_content = match fs::read(legacy_file) {
        Ok(content) => content,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            debug!(
                "[host-store][migration] Legacy hosts file is absent; migration skipped: {}",
                legacy_file.display()
            );
            return Ok(());
        }
        Err(error) => {
            return Err(AppError::StorageError(ErrorDetail::msg(
                "读取旧主机配置失败: {0}",
                vec![error.to_string()],
            )));
        }
    };

    let mut new_hosts = match OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(new_file)
    {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::AlreadyExists => {
            debug!(
                "[host-store][migration] Production hosts file already exists; migration skipped: {}",
                new_file.display()
            );
            return Ok(());
        }
        Err(error) => {
            return Err(AppError::StorageError(ErrorDetail::msg(
                "创建迁移后的主机配置失败: {0}",
                vec![error.to_string()],
            )));
        }
    };

    if let Err(error) = new_hosts.write_all(&legacy_content) {
        // Windows 不允许删除仍被本进程持有的文件，先关闭再清理部分写入结果。
        drop(new_hosts);
        let _ = fs::remove_file(new_file);
        return Err(AppError::StorageError(ErrorDetail::msg(
            "迁移旧主机配置失败: {0}",
            vec![error.to_string()],
        )));
    }

    info!(
        "[host-store][migration] Migrated legacy hosts file: {} -> {}",
        legacy_file.display(),
        new_file.display()
    );
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
        let app_data_dir = app_handle.path().app_data_dir().map_err(|error| {
            AppError::StorageError(ErrorDetail::msg(
                "无法获取应用数据目录: {0}",
                vec![error.to_string()],
            ))
        })?;

        // 确保数据目录存在，首次运行时自动创建
        fs::create_dir_all(&app_data_dir).map_err(|error| {
            AppError::StorageError(ErrorDetail::msg(
                "无法创建应用数据目录: {0}",
                vec![error.to_string()],
            ))
        })?;

        let file_path = app_data_dir.join(HOSTS_FILE_NAME);
        if let Some(data_root) = app_data_dir.parent() {
            let legacy_file = data_root.join(LEGACY_IDENTIFIER).join(HOSTS_FILE_NAME);
            if let Err(error) = migrate_legacy_hosts(&legacy_file, &file_path) {
                // 旧配置仅是可选的一次性迁移源；失败不能阻断正式存储的首次使用，
                // 下次启动仍会重试，诊断保留在日志中供排查。
                warn!(
                    "[host-store][migration] Legacy hosts migration failed; continuing with production store: {}",
                    error
                );
            }
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
    /// 文件读取期间消失同样返回空列表；内容非法时隔离损坏文件后返回 StorageError。
    ///
    /// # 返回
    /// 成功返回主机配置列表，失败返回 StorageError
    pub fn load(&self) -> Result<Vec<HostConfig>, AppError> {
        let content = match fs::read_to_string(&self.file_path) {
            Ok(content) => content,
            // 不预先检查 exists，避免文件在检查和读取之间消失时被误报为存储错误。
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => {
                return Err(AppError::StorageError(ErrorDetail::msg(
                    "读取主机配置文件失败: {0}",
                    vec![error.to_string()],
                )));
            }
        };

        match serde_json::from_str(&content) {
            Ok(hosts) => Ok(hosts),
            Err(parse_error) => {
                let backup_path = corrupt_hosts_backup_path(&self.file_path);
                match fs::rename(&self.file_path, &backup_path) {
                    Ok(()) => Err(AppError::StorageError(ErrorDetail::msg(
                        "解析主机配置文件失败: {0}；损坏文件已隔离至: {1}",
                        vec![parse_error.to_string(), backup_path.display().to_string()],
                    ))),
                    Err(quarantine_error) => Err(AppError::StorageError(ErrorDetail::msg(
                        "解析主机配置文件失败: {0}；隔离损坏文件失败: {1}",
                        vec![parse_error.to_string(), quarantine_error.to_string()],
                    ))),
                }
            }
        }
    }

    /// 将主机配置列表持久化到 hosts.json
    ///
    /// 使用 pretty-print JSON 格式写入，便于人工排查问题。
    /// 写入前不含任何明文凭据，调用方必须确保已完成凭据剥离。
    ///
    /// 原子写：先写同目录临时文件再 rename 覆盖，失败/崩溃时 hosts.json
    /// 要么是旧内容要么是新内容，不会出现截断或半写状态。进程内并发已由
    /// SharedHostConfigService 互斥锁串行化整个 load-modify-write 周期。
    ///
    /// # 参数
    /// - `hosts`: 要持久化的主机配置切片（不含明文凭据）
    pub fn save(&self, hosts: &[HostConfig]) -> Result<(), AppError> {
        let content = serde_json::to_string_pretty(hosts).map_err(|error| {
            AppError::StorageError(ErrorDetail::msg(
                "序列化主机配置失败: {0}",
                vec![error.to_string()],
            ))
        })?;

        // ponytail: 跨进程文件锁未加；桌面应用单实例下 rename 原子性已保证
        // 无损坏，若支持多实例共享配置目录再加锁。
        let tmp_path = self.file_path.with_extension("tmp");
        let write_result =
            fs::write(&tmp_path, &content).and_then(|()| fs::rename(&tmp_path, &self.file_path));
        if let Err(error) = write_result {
            // 尽力清理临时文件，失败只留孤儿 tmp 条目，不阻断错误上报
            let _ = fs::remove_file(&tmp_path);
            return Err(AppError::StorageError(ErrorDetail::msg(
                "写入主机配置文件失败: {0}",
                vec![error.to_string()],
            )));
        }

        Ok(())
    }
}

#[cfg(test)]
#[path = "host_store_test.rs"]
mod tests;
