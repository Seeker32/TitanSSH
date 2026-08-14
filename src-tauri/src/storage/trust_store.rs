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

use crate::errors::app_error::AppError;
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
        let app_data_dir = app_handle
            .path()
            .app_data_dir()
            .map_err(|error| AppError::TrustStoreError(format!("无法获取应用数据目录: {error}")))?;
        fs::create_dir_all(&app_data_dir)
            .map_err(|error| AppError::TrustStoreError(format!("无法创建应用数据目录: {error}")))?;
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
            return Err(AppError::TrustStoreError(format!(
                "读取信任存储失败: {} ({error})",
                file_path.display()
            )));
        }
    };
    let mut records = Vec::new();
    for (index, line) in content.lines().enumerate() {
        if let Some(record) = parse_entry(line).map_err(|reason| {
            AppError::TrustStoreError(format!(
                "解析信任存储失败: {} 第 {} 行 ({reason})",
                file_path.display(),
                index + 1
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
        AppError::TrustStoreError(format!("信任存储路径无父目录: {}", file_path.display()))
    })?;
    let mut temp = NamedTempFile::new_in(dir)
        .map_err(|error| AppError::TrustStoreError(format!("创建信任存储临时文件失败: {error}")))?;
    temp.write_all(content.as_bytes())
        .map_err(|error| AppError::TrustStoreError(format!("写入信任存储临时文件失败: {error}")))?;
    temp.as_file()
        .sync_all()
        .map_err(|error| AppError::TrustStoreError(format!("同步信任存储临时文件失败: {error}")))?;
    temp.persist(file_path).map_err(|error| {
        AppError::TrustStoreError(format!(
            "发布信任存储失败: {} ({})，原文件未受影响",
            file_path.display(),
            error.error
        ))
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::thread;
    use uuid::Uuid;

    /// 在系统临时目录创建隔离的测试文件路径。
    fn temp_store_path() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("titan-trust-store-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).expect("temp dir should be created");
        dir.join(KNOWN_HOSTS_FILE_NAME)
    }

    fn store_at(path: &Path) -> TrustStore {
        TrustStore::from_file_path(path.to_path_buf())
    }

    fn record(host: &str, port: u16, algorithm: &str, blob: &[u8]) -> TrustRecord {
        TrustRecord {
            host: host.to_string(),
            port,
            algorithm: algorithm.to_string(),
            blob: blob.to_vec(),
        }
    }

    /// 缺失文件视为空信任存储，不产生错误。
    #[test]
    fn missing_file_is_empty_trust_store() {
        let path = temp_store_path();
        let store = store_at(&path);
        assert_eq!(store.lookup("10.0.0.8", 22).unwrap(), None);
    }

    /// 保存后按精确 endpoint 匹配读取，公钥 blob 字节级往返一致。
    #[test]
    fn save_and_lookup_round_trip_exact_key_material() {
        let path = temp_store_path();
        let store = store_at(&path);
        let blob = b"openssh-wire-blob-bytes".to_vec();
        store
            .upsert(record("10.0.0.8", 22, "ssh-ed25519", &blob))
            .unwrap();

        let found = store.lookup("10.0.0.8", 22).unwrap().unwrap();
        assert_eq!(found.host, "10.0.0.8");
        assert_eq!(found.port, 22);
        assert_eq!(found.algorithm, "ssh-ed25519");
        assert_eq!(found.blob, blob);
        assert!(found.matches("10.0.0.8", 22, "ssh-ed25519", &blob));
    }

    /// 端口 22 的主机不带括号；磁盘内容为标准 OpenSSH 表示。
    #[test]
    fn default_port_uses_plain_host_pattern() {
        let path = temp_store_path();
        let store = store_at(&path);
        store
            .upsert(record("prod.example.com", 22, "ssh-rsa", b"blob"))
            .unwrap();
        let content = fs::read_to_string(&path).unwrap();
        assert_eq!(content, "prod.example.com ssh-rsa YmxvYg\n");
    }

    /// 非 22 端口使用 `[host]:port` 标准表示，重新加载后端口精确还原。
    #[test]
    fn non_default_port_uses_bracket_notation() {
        let path = temp_store_path();
        let store = store_at(&path);
        store
            .upsert(record("10.0.0.8", 2222, "ssh-ed25519", b"blob"))
            .unwrap();
        let content = fs::read_to_string(&path).unwrap();
        assert_eq!(content, "[10.0.0.8]:2222 ssh-ed25519 YmxvYg\n");
        let found = store.lookup("10.0.0.8", 2222).unwrap().unwrap();
        assert_eq!(found.port, 2222);
        // 同一主机不同端口互不干扰
        assert_eq!(store.lookup("10.0.0.8", 22).unwrap(), None);
    }

    /// IPv6 地址（含端口 22）使用 `[addr]:port` 标准表示并精确还原。
    #[test]
    fn ipv6_uses_bracket_notation_even_on_default_port() {
        let path = temp_store_path();
        let store = store_at(&path);
        store
            .upsert(record("2001:db8::1", 22, "ssh-ed25519", b"blob"))
            .unwrap();
        let content = fs::read_to_string(&path).unwrap();
        assert_eq!(content, "[2001:db8::1]:22 ssh-ed25519 YmxvYg\n");
        let found = store.lookup("2001:db8::1", 22).unwrap().unwrap();
        assert_eq!(found.host, "2001:db8::1");
    }

    /// endpoint 拼写精确保留：不做小写、尾点归一化，不同拼写是不同 endpoint。
    #[test]
    fn endpoint_spelling_is_exact() {
        let path = temp_store_path();
        let store = store_at(&path);
        store
            .upsert(record("Prod.Example.COM", 22, "ssh-ed25519", b"blob"))
            .unwrap();
        store
            .upsert(record("prod.example.com", 22, "ssh-ed25519", b"other"))
            .unwrap();
        store
            .upsert(record("example.com.", 22, "ssh-ed25519", b"trailing"))
            .unwrap();
        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("Prod.Example.COM ssh-ed25519 YmxvYg\n"));
        assert!(content.contains("prod.example.com ssh-ed25519 b3RoZXI\n"));
        assert!(content.contains("example.com. ssh-ed25519 dHJhaWxpbmc\n"));
    }

    /// 同一 endpoint 再次 upsert 只保留最新记录（一个当前算法 + 完整公钥）。
    #[test]
    fn upsert_keeps_single_record_per_endpoint() {
        let path = temp_store_path();
        let store = store_at(&path);
        store
            .upsert(record("10.0.0.8", 22, "ssh-rsa", b"old"))
            .unwrap();
        store
            .upsert(record("10.0.0.8", 22, "ssh-ed25519", b"new"))
            .unwrap();
        let found = store.lookup("10.0.0.8", 22).unwrap().unwrap();
        assert_eq!(found.algorithm, "ssh-ed25519");
        assert_eq!(found.blob, b"new");
        assert_eq!(
            fs::read_to_string(&path).unwrap().lines().count(),
            1,
            "同一 endpoint 只保留一条记录"
        );
    }

    /// 不可解析文件 fail-closed：load 返回 TrustStoreError，绝不静默视为空。
    #[test]
    fn corrupt_file_fails_closed() {
        let path = temp_store_path();
        fs::write(&path, "10.0.0.8 ssh-ed25519\n").unwrap();
        let store = store_at(&path);
        let error = store.lookup("10.0.0.8", 22).unwrap_err();
        assert_eq!(error.code(), "TrustStoreError");

        // 非法 base64 同样 fail-closed
        let path2 = temp_store_path();
        fs::write(&path2, "10.0.0.8 ssh-ed25519 not-base64!!!\n").unwrap();
        let store2 = store_at(&path2);
        assert_eq!(
            store2.lookup("10.0.0.8", 22).unwrap_err().code(),
            "TrustStoreError"
        );

        // 无括号含冒号的模糊主机段也 fail-closed
        let path3 = temp_store_path();
        fs::write(&path3, "::1 ssh-ed25519 blob\n").unwrap();
        let store3 = store_at(&path3);
        assert_eq!(
            store3.lookup("::1", 22).unwrap_err().code(),
            "TrustStoreError"
        );
    }

    /// 不可读文件（路径为目录）fail-closed。
    #[test]
    fn unreadable_file_fails_closed() {
        let dir = std::env::temp_dir().join(format!("titan-trust-dir-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).expect("temp dir should be created");
        let store = TrustStore::from_file_path(dir.clone());
        let error = store.lookup("10.0.0.8", 22).unwrap_err();
        assert_eq!(error.code(), "TrustStoreError");
    }

    /// 写入失败（目标为目录）返回错误，且缓存保持原状：失败的记录不会
    /// 被误认为已持久化，原有记录仍可读取。
    #[test]
    fn write_failure_reports_error_and_keeps_cache_unchanged() {
        let path = temp_store_path();
        let store = store_at(&path);
        store
            .upsert(record("10.0.0.8", 22, "ssh-ed25519", b"old"))
            .unwrap();
        // 缓存已加载后破坏文件路径：目标替换为目录，发布必然失败
        fs::remove_file(&path).unwrap();
        fs::create_dir_all(&path).unwrap();
        let error = store
            .upsert(record("10.0.0.8", 22, "ssh-ed25519", b"new"))
            .unwrap_err();
        assert_eq!(error.code(), "TrustStoreError");
        // 缓存不被失败写入污染：仍返回旧记录（与磁盘最后一次成功发布一致）
        assert_eq!(store.lookup("10.0.0.8", 22).unwrap().unwrap().blob, b"old");
    }

    /// 空行与注释行跳过，其余行正常解析。
    #[test]
    fn blank_lines_and_comments_are_skipped() {
        let path = temp_store_path();
        fs::write(
            &path,
            "# TitanSSH trust store\n\n10.0.0.8 ssh-ed25519 blob\n\n",
        )
        .unwrap();
        let store = store_at(&path);
        let records = store.reload().unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].host, "10.0.0.8");
        assert_eq!(records[0].port, 22);
    }

    /// 并发 upsert 不同 endpoint：串行化 + 读改写不丢失任何记录。
    #[test]
    fn concurrent_upserts_preserve_all_records() {
        let path = temp_store_path();
        let store = store_at(&path);
        let handles: Vec<_> = (0..8)
            .map(|index| {
                let store = store.clone();
                thread::spawn(move || {
                    store
                        .upsert(record(
                            &format!("10.0.0.{index}"),
                            22,
                            "ssh-ed25519",
                            format!("blob-{index}").as_bytes(),
                        ))
                        .unwrap();
                })
            })
            .collect();
        for handle in handles {
            handle.join().unwrap();
        }
        let records = store.reload().unwrap();
        assert_eq!(records.len(), 8, "并发保存不得丢失任何 endpoint 记录");
        for index in 0..8 {
            let found = store
                .lookup(&format!("10.0.0.{index}"), 22)
                .unwrap()
                .unwrap();
            assert_eq!(found.blob, format!("blob-{index}").as_bytes());
        }
    }

    /// 同 endpoint 并发 upsert：最终只保留一条记录（最后写入者胜出），文件不损坏。
    #[test]
    fn concurrent_upserts_same_endpoint_keep_single_record() {
        let path = temp_store_path();
        let store = store_at(&path);
        let handles: Vec<_> = (0..6)
            .map(|index| {
                let store = store.clone();
                thread::spawn(move || {
                    store
                        .upsert(record(
                            "10.0.0.8",
                            22,
                            "ssh-ed25519",
                            format!("blob-{index}").as_bytes(),
                        ))
                        .unwrap();
                })
            })
            .collect();
        for handle in handles {
            handle.join().unwrap();
        }
        let records = store.reload().unwrap();
        assert_eq!(records.len(), 1);
        assert!(
            records[0].blob.starts_with(b"blob-"),
            "最终记录必须是某一次完整写入"
        );
    }

    /// 生产构造路径：通过 AppHandle 解析应用数据目录并定位 known_hosts 文件。
    /// mock app 的应用数据目录在测试间共享：使用唯一 host 避免与其他测试的
    /// init_trust_store 写入互相干扰。
    #[test]
    fn new_resolves_app_data_dir_path() {
        let app = tauri::test::mock_app();
        let store = TrustStore::new(&app.handle()).expect("mock app 应可解析应用数据目录");
        let unique_host = format!(
            "10.0.0.{}",
            uuid::Uuid::new_v4().simple().to_string()[..8].to_string()
        );
        store
            .upsert(record(&unique_host, 22, "ssh-ed25519", b"blob"))
            .expect("生产路径写入应成功");
        let found = store.lookup(&unique_host, 22).unwrap().unwrap();
        assert_eq!(found.blob, b"blob");
    }

    /// 主机段格式序列化遵循 OpenSSH 规则（单元级向量）。
    #[test]
    fn host_pattern_serialization_follows_openssh_rules() {
        assert_eq!(format_host_pattern("10.0.0.8", 22), "10.0.0.8");
        assert_eq!(format_host_pattern("10.0.0.8", 2222), "[10.0.0.8]:2222");
        assert_eq!(format_host_pattern("::1", 22), "[::1]:22");
        assert_eq!(
            format_host_pattern("2001:db8::1", 2200),
            "[2001:db8::1]:2200"
        );
        assert_eq!(
            parse_host_pattern("10.0.0.8").unwrap(),
            ("10.0.0.8".to_string(), 22)
        );
        assert_eq!(
            parse_host_pattern("[10.0.0.8]:2222").unwrap(),
            ("10.0.0.8".to_string(), 2222)
        );
        assert_eq!(
            parse_host_pattern("[::1]:22").unwrap(),
            ("::1".to_string(), 22)
        );
        assert!(parse_host_pattern("[10.0.0.8]").is_err());
        assert!(parse_host_pattern("[10.0.0.8]:notaport").is_err());
        assert!(parse_host_pattern("10.0.0.8:2222").is_err());
        assert!(parse_host_pattern("").is_err());
    }
}
