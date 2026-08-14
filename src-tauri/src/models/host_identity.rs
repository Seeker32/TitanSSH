use serde::{Deserialize, Serialize};

/// Settings“可信主机”只读清单条目：精确 endpoint 的当前算法与 SHA-256 指纹。
///
/// 原始公钥 material 只存在后端；指纹由后端从公钥 blob 计算，
/// 前端只消费 typed JSON，绝不解析 known_hosts 文本。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TrustedHostInfo {
    pub host: String,
    pub port: u16,
    pub algorithm: String,
    pub fingerprint: String,
}
