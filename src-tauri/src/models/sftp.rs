use serde::{Deserialize, Serialize};

/// 远程文件系统条目（文件或目录）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteEntry {
    /// 文件或目录名称（不含路径）
    pub name: String,
    /// 完整绝对路径
    pub path: String,
    /// 是否为目录
    pub is_dir: bool,
    /// 文件大小（bytes），目录为 0
    pub size: u64,
    /// 最后修改时间（Unix 毫秒时间戳）
    pub modified_at: i64,
    /// 权限字符串，如 "rwxr-xr-x"
    pub permissions: String,
}

/// 传输方向
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum TransferType {
    Upload,
    Download,
}

/// SFTP 任务专用状态枚举，增加 Cancelled 变体以区分主动取消与失败
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum SftpTaskStatus {
    Pending,
    Running,
    Done,
    Failed,
    Cancelled,
}

/// 传输任务完整状态；初始 status 为 Pending
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransferTask {
    /// 全局唯一任务 ID（UUID v4）
    pub task_id: String,
    /// 关联的 SSH 会话 ID
    pub session_id: String,
    /// 传输方向
    pub transfer_type: TransferType,
    /// 远程文件完整路径
    pub remote_path: String,
    /// 本地文件完整路径
    pub local_path: String,
    /// 文件名（从路径提取，用于 UI 展示）
    pub file_name: String,
    /// 文件总大小（bytes）
    pub total_bytes: u64,
    /// 已传输字节数
    pub transferred_bytes: u64,
    /// 当前传输速度（bytes/s）
    pub speed_bps: u64,
    /// 任务状态
    pub status: SftpTaskStatus,
    /// 失败原因；status = Failed 时为具体错误描述，status = Cancelled 时为 None
    pub error_message: Option<String>,
    /// 任务创建时间（Unix 毫秒时间戳）
    pub created_at: i64,
}

/// sftp:progress 事件 payload，约每 500ms 推送一次
#[derive(Debug, Clone, Serialize)]
pub struct SftpProgressEvent {
    /// 任务 ID
    pub task_id: String,
    /// 关联会话 ID
    pub session_id: String,
    /// 已传输字节数
    pub transferred_bytes: u64,
    /// 文件总大小（bytes）
    pub total_bytes: u64,
    /// 当前传输速度（bytes/s）
    pub speed_bps: u64,
}

/// sftp:task_status 事件 payload，任务状态变更时推送
#[derive(Debug, Clone, Serialize)]
pub struct SftpTaskStatusEvent {
    /// 任务 ID
    pub task_id: String,
    /// 关联会话 ID
    pub session_id: String,
    /// 新状态
    pub status: SftpTaskStatus,
    /// 失败原因；status = Failed 时为具体错误描述，status = Cancelled 时为 None
    pub error_message: Option<String>,
}
