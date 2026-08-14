use crate::errors::app_error::AppErrorInfo;
use serde::{Deserialize, Serialize};

/// 远程文件系统条目（文件或目录）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
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

/// 传输最终目标已存在时的冲突处理策略（上传与下载共用）；未显式指定时默认 Reject。
///
/// Reject 绝不覆盖已有目标文件；Overwrite 仅在用户逐文件确认后使用，
/// 经同目录临时文件安全发布到最终目标，失败不破坏原文件。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum ConflictStrategy {
    /// 目标已存在时拒绝，返回结构化 SftpTargetExists 错误
    #[default]
    Reject,
    /// 目标已存在时安全替换
    Overwrite,
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
#[serde(rename_all = "camelCase")]
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
    /// 失败原因；status = Failed 或取消后临时文件清理失败时为具体错误描述，其余为 None
    pub error: Option<AppErrorInfo>,
    /// 任务创建时间（Unix 毫秒时间戳）
    pub created_at: i64,
}

/// sftp:progress 事件 payload，约每 500ms 推送一次
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SftpTaskStatusEvent {
    /// 任务 ID
    pub task_id: String,
    /// 关联会话 ID
    pub session_id: String,
    /// 新状态
    pub status: SftpTaskStatus,
    /// 失败原因；status = Failed 或取消后临时文件清理失败时为具体错误描述，其余为 None
    pub error: Option<AppErrorInfo>,
}

#[cfg(test)]
mod tests {
    use super::ConflictStrategy;

    /// 冲突策略默认值为 Reject：未显式指定时绝不覆盖本地文件。
    #[test]
    fn conflict_strategy_defaults_to_reject() {
        assert_eq!(ConflictStrategy::default(), ConflictStrategy::Reject);
    }

    /// IPC 载荷与 TransferType/TaskStatus 同约定：PascalCase 字符串往返。
    #[test]
    fn conflict_strategy_roundtrips_pascal_case() {
        let value: ConflictStrategy = serde_json::from_str("\"Overwrite\"").unwrap();
        assert_eq!(value, ConflictStrategy::Overwrite);
        let value: ConflictStrategy = serde_json::from_str("\"Reject\"").unwrap();
        assert_eq!(value, ConflictStrategy::Reject);
        assert_eq!(
            serde_json::to_string(&ConflictStrategy::Overwrite).unwrap(),
            "\"Overwrite\""
        );
    }
}
