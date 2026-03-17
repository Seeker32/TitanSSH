use serde::{Deserialize, Serialize};

/// 主机配置，用于持久化存储与展示，不含明文凭据
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HostConfig {
    pub id: String,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub auth_type: AuthType,
    /// 密码在安全存储中的引用键，不含明文
    pub password_ref: Option<String>,
    pub private_key_path: Option<String>,
    /// 私钥口令在安全存储中的引用键，不含明文
    pub passphrase_ref: Option<String>,
    pub remark: Option<String>,
}

/// 保存主机请求，仅用于接收前端提交的明文凭据，后端落盘前必须清除明文字段
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SaveHostRequest {
    pub id: String,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub auth_type: AuthType,
    /// 明文密码，仅在请求中存在，不得落盘
    pub password: Option<String>,
    pub private_key_path: Option<String>,
    /// 明文私钥口令，仅在请求中存在，不得落盘
    pub passphrase: Option<String>,
    pub remark: Option<String>,
}

/// 认证类型枚举
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum AuthType {
    Password,
    PrivateKey,
}
