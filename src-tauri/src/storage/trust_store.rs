//! TitanSSH 独立信任存储：应用数据目录下的标准 OpenSSH `known_hosts` 文件。
//!
//! 只读写 TitanSSH 自有文件，不触碰系统 `~/.ssh/known_hosts`，也不使用 keyring。
//! 每个精确 `host + port` 只保存一条记录（当前算法 + 完整公钥 blob）；读取与写入
//! 在同一把锁内串行化，写入采用同目录临时文件安全发布，失败不改动缓存与磁盘内容。
//!
//! 语义约定：
//! - 文件缺失 = 空信任存储（首次运行场景）；
//! - 文件不可读 / 无法解析 = fail-closed 错误，绝不静默视为空；
//! - endpoint 不做小写、尾点、别名或解析 IP 合并，保留配置中的精确拼写。

use crate::errors::app_error::{AppError, ErrorDetail};
use base64::Engine;
use base64::engine::general_purpose::STANDARD_NO_PAD;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Manager};
use tempfile::NamedTempFile;

/// TitanSSH 自有信任文件名（标准 OpenSSH known_hosts 格式）。
const KNOWN_HOSTS_FILE_NAME: &str = "known_hosts";

/// 单条 endpoint 信任记录：精确 host + port → 当前算法 + 完整公钥。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustRecord {
    pub host: String,
    pub port: u16,
    /// OpenSSH 风格算法名（如 ssh-ed25519）
    pub algorithm: String,
    /// ssh2 提供的原始主机公钥 blob（OpenSSH wire 格式）；指纹由后端从它计算
    pub blob: Vec<u8>,
}

impl TrustRecord {
    /// 判断记录是否与呈现的 endpoint + 算法 + 完整公钥精确匹配。
    pub fn matches(&self, host: &str, port: u16, algorithm: &str, blob: &[u8]) -> bool {
        self.host == host && self.port == port && self.algorithm == algorithm && self.blob == blob
    }
}

/// 文件内部可变状态：路径 + 已加载记录缓存（None = 尚未成功加载）。
struct TrustStoreState {
    file_path: PathBuf,
    records: Option<Vec<TrustRecord>>,
}

/// 线程安全的信任存储；读写全部在内部锁内串行化，并发保存不丢失记录。
#[derive(Clone)]
pub struct TrustStore {
    state: Arc<Mutex<TrustStoreState>>,
}

impl TrustStore {
    /// 通过 Tauri AppHandle 解析应用数据目录并定位 known_hosts 文件。
    ///
    /// 首次运行自动创建数据目录；目录创建失败返回 StorageError。
    pub fn new<R: tauri::Runtime>(app_handle: &AppHandle<R>) -> Result<Self, AppError> {
        let app_data_dir = app_handle.path().app_data_dir().map_err(|error| {
            AppError::TrustStoreError(ErrorDetail::msg(
                "无法获取应用数据目录: {0}",
                vec![error.to_string()],
            ))
        })?;
        fs::create_dir_all(&app_data_dir).map_err(|error| {
            AppError::TrustStoreError(ErrorDetail::msg(
                "无法创建应用数据目录: {0}",
                vec![error.to_string()],
            ))
        })?;
        Ok(Self::from_file_path(
            app_data_dir.join(KNOWN_HOSTS_FILE_NAME),
        ))
    }

    /// 直接通过文件路径构造（测试与自定义路径使用）。
    pub(crate) fn from_file_path(file_path: PathBuf) -> Self {
        Self {
            state: Arc::new(Mutex::new(TrustStoreState {
                file_path,
                records: None,
            })),
        }
    }

    /// 查询精确 endpoint 的信任记录。
    ///
    /// 文件缺失返回 Ok(None)（空信任存储）；不可读或解析失败返回
    /// TrustStoreError（fail-closed），调用方必须终止连接而非创建 challenge。
    pub fn lookup(&self, host: &str, port: u16) -> Result<Option<TrustRecord>, AppError> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let records = ensure_loaded(&mut state)?;
        Ok(records
            .iter()
            .find(|record| record.host == host && record.port == port)
            .cloned())
    }

    /// 写入或替换精确 endpoint 的信任记录（每个 host + port 至多一条）。
    ///
    /// 在锁内完成 加载 → 修改 → 安全发布 → 更新缓存：并发 upsert 串行执行，
    /// 不会丢失其他 endpoint 的记录；磁盘写入失败时缓存保持原状并返回错误。
    pub fn upsert(&self, record: TrustRecord) -> Result<(), AppError> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut records = ensure_loaded(&mut state)?.to_vec();
        records.retain(|existing| !(existing.host == record.host && existing.port == record.port));
        records.push(record);
        write_records(&state.file_path, &records)?;
        state.records = Some(records);
        Ok(())
    }

    /// 移除精确 endpoint 的信任记录；endpoint 不存在时静默成功（幂等）。
    ///
    /// 与 upsert 一样在锁内完成 加载 → 修改 → 安全发布 → 更新缓存；
    /// 磁盘写入失败时缓存保持原状并返回错误（不得让调用方误以为已删除）。
    pub fn remove(&self, host: &str, port: u16) -> Result<(), AppError> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut records = ensure_loaded(&mut state)?.to_vec();
        let before = records.len();
        records.retain(|record| !(record.host == host && record.port == port));
        if records.len() == before {
            // endpoint 本无记录：无需发布，保持幂等
            return Ok(());
        }
        write_records(&state.file_path, &records)?;
        state.records = Some(records);
        Ok(())
    }

    /// 列出全部信任记录，按 host 字典序 + port 稳定排序（Settings 清单展示顺序）。
    ///
    /// 文件缺失返回空列表；不可读或解析失败返回 TrustStoreError（fail-closed），
    /// 绝不把错误伪装成空列表。
    pub fn list(&self) -> Result<Vec<TrustRecord>, AppError> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let records = ensure_loaded(&mut state)?;
        let mut sorted = records.to_vec();
        sorted.sort_by(|a, b| a.host.cmp(&b.host).then(a.port.cmp(&b.port)));
        Ok(sorted)
    }

    /// 重新读取磁盘内容（测试用：观察真实文件状态，绕开内存缓存）。
    #[cfg(test)]
    pub(crate) fn reload(&self) -> Result<Vec<TrustRecord>, AppError> {
        let state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        load_from_file(&state.file_path)
    }
}

/// 保证内存缓存已加载：首次访问读取文件，之后复用缓存。
fn ensure_loaded(state: &mut TrustStoreState) -> Result<&[TrustRecord], AppError> {
    if state.records.is_none() {
        state.records = Some(load_from_file(&state.file_path)?);
    }
    Ok(state.records.as_deref().unwrap_or(&[]))
}

/// 读取并解析 known_hosts 文件。
///
/// 文件不存在返回空列表；空行与 `#` 注释行跳过；其余每一行必须完整解析，
/// 任何失败返回 TrustStoreError（fail-closed，不静默视为空）。
/// 读取并解析 known_hosts 文件。
///
/// 仅文件确实不存在（NotFound）时返回空列表；其余任何读取错误（权限、IO 等）
/// 返回 TrustStoreError（fail-closed，不静默视为空）。空行与 `#` 注释行跳过，
/// 其余每一行必须完整解析，任何失败同样 fail-closed。
fn load_from_file(file_path: &Path) -> Result<Vec<TrustRecord>, AppError> {
    let content = match fs::read_to_string(file_path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(AppError::TrustStoreError(ErrorDetail::msg(
                "读取信任存储失败: {0} ({1})",
                vec![file_path.display().to_string(), error.to_string()],
            )));
        }
    };
    let mut records = Vec::new();
    for (index, line) in content.lines().enumerate() {
        if let Some(record) = parse_entry(line).map_err(|reason| {
            AppError::TrustStoreError(ErrorDetail::msg(
                "解析信任存储失败: {0} 第 {1} 行 ({2})",
                vec![
                    file_path.display().to_string(),
                    (index + 1).to_string(),
                    reason,
                ],
            ))
        })? {
            records.push(record);
        }
    }
    Ok(records)
}

/// 解析单行记录；空行与注释返回 None，其余必须为
/// `<host-pattern> <algorithm> <base64 blob>`。
fn parse_entry(line: &str) -> Result<Option<TrustRecord>, String> {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return Ok(None);
    }
    let mut parts = trimmed.split_whitespace();
    let (Some(pattern), Some(algorithm), Some(encoded), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err("期望 `<host> <algorithm> <base64>` 三段".to_string());
    };
    let (host, port) = parse_host_pattern(pattern)?;
    if algorithm.is_empty() {
        return Err("算法名不能为空".to_string());
    }
    if encoded.is_empty() {
        return Err("公钥不能为空".to_string());
    }
    // 兼容带与不带 '=' 填充的标准 base64：去掉填充后按无填充解码
    let blob = STANDARD_NO_PAD
        .decode(encoded.trim_end_matches('='))
        .map_err(|error| format!("公钥 base64 解码失败 ({error})"))?;
    if blob.is_empty() {
        return Err("公钥解码后为空".to_string());
    }
    Ok(Some(TrustRecord {
        host,
        port,
        algorithm: algorithm.to_string(),
        blob,
    }))
}

/// 解析 `<host>` 或 `[host]:port` 主机段，返回精确 (host, port)。
///
/// 与 OpenSSH 写入规则一致：非 22 端口或含冒号（IPv6）的地址使用
/// `[host]:port`；无括号且无冒号的地址视为端口 22。无括号却含冒号的
/// 形式本应用从不写入且语义模糊，按解析失败处理（fail-closed）。
fn parse_host_pattern(pattern: &str) -> Result<(String, u16), String> {
    if let Some(rest) = pattern.strip_prefix('[') {
        let close = rest
            .find(']')
            .ok_or_else(|| format!("主机段缺少右括号: {pattern}"))?;
        let host = &rest[..close];
        let port_text = rest[close + 1..]
            .strip_prefix(':')
            .ok_or_else(|| format!("括号主机段缺少端口: {pattern}"))?;
        if host.is_empty() {
            return Err(format!("主机段主机为空: {pattern}"));
        }
        let port: u16 = port_text
            .parse()
            .map_err(|error| format!("端口解析失败: {pattern} ({error})"))?;
        Ok((host.to_string(), port))
    } else if pattern.contains(':') {
        Err(format!("主机段格式非法（无括号含冒号）: {pattern}"))
    } else if pattern.is_empty() {
        Err("主机段为空".to_string())
    } else {
        Ok((pattern.to_string(), 22))
    }
}

/// 将 endpoint 序列化为标准 OpenSSH known_hosts 主机段。
///
/// 非 22 端口或含冒号的 IPv6 地址使用 `[host]:port`，其余保持原拼写（不进行
/// 小写、尾点、别名或解析 IP 归一化）。
fn format_host_pattern(host: &str, port: u16) -> String {
    if port != 22 || host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        host.to_string()
    }
}

/// 将全部记录序列化为 known_hosts 文件内容。
fn serialize_records(records: &[TrustRecord]) -> String {
    let mut content = String::new();
    for record in records {
        content.push_str(&format!(
            "{} {} {}\n",
            format_host_pattern(&record.host, record.port),
            record.algorithm,
            STANDARD_NO_PAD.encode(&record.blob)
        ));
    }
    content
}

/// 安全发布：全部内容先写入同目录唯一临时文件，flush + sync 成功后原子替换目标。
///
/// POSIX rename / Windows MoveFileEx REPLACE_EXISTING 语义：发布失败不改动原文件。
fn write_records(file_path: &Path, records: &[TrustRecord]) -> Result<(), AppError> {
    let content = serialize_records(records);
    let dir = file_path.parent().ok_or_else(|| {
        AppError::TrustStoreError(ErrorDetail::msg(
            "信任存储路径无父目录: {0}",
            vec![file_path.display().to_string()],
        ))
    })?;
    let mut temp = NamedTempFile::new_in(dir).map_err(|error| {
        AppError::TrustStoreError(ErrorDetail::msg(
            "创建信任存储临时文件失败: {0}",
            vec![error.to_string()],
        ))
    })?;
    temp.write_all(content.as_bytes()).map_err(|error| {
        AppError::TrustStoreError(ErrorDetail::msg(
            "写入信任存储临时文件失败: {0}",
            vec![error.to_string()],
        ))
    })?;
    temp.as_file().sync_all().map_err(|error| {
        AppError::TrustStoreError(ErrorDetail::msg(
            "同步信任存储临时文件失败: {0}",
            vec![error.to_string()],
        ))
    })?;
    temp.persist(file_path).map_err(|error| {
        AppError::TrustStoreError(ErrorDetail::msg(
            "发布信任存储失败: {0} ({1})，原文件未受影响",
            vec![file_path.display().to_string(), error.error.to_string()],
        ))
    })?;
    Ok(())
}

#[cfg(test)]
#[path = "trust_store_test.rs"]
mod tests;
