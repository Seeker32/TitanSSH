use serde::{Deserialize, Serialize};

/// 主机配置，用于持久化存储与展示，不含明文凭据
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HostConfig {
    pub id: String,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    #[serde(alias = "auth_type")]
    pub auth_type: AuthType,
    /// 密码在安全存储中的引用键，不含明文
    #[serde(alias = "password_ref")]
    pub password_ref: Option<String>,
    #[serde(alias = "private_key_path")]
    pub private_key_path: Option<String>,
    /// 私钥口令在安全存储中的引用键，不含明文
    #[serde(alias = "passphrase_ref")]
    pub passphrase_ref: Option<String>,
    pub remark: Option<String>,
    /// 分组名，空串表示"未分组"
    #[serde(default)]
    pub group: String,
}

/// 保存主机请求，仅用于接收前端提交的明文凭据，后端落盘前必须清除明文字段
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveHostRequest {
    pub id: String,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    #[serde(alias = "auth_type")]
    pub auth_type: AuthType,
    /// 明文密码三态输入，仅在请求中存在，不得落盘
    pub password: Option<CredentialInput>,
    #[serde(alias = "private_key_path")]
    pub private_key_path: Option<String>,
    /// 明文私钥口令三态输入，仅在请求中存在，不得落盘
    pub passphrase: Option<CredentialInput>,
    pub remark: Option<String>,
    /// 分组名，空串表示"未分组"
    #[serde(default)]
    pub group: String,
}

/// 认证类型枚举
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum AuthType {
    Password,
    PrivateKey,
}

/// 保存请求中的三态凭据输入：显式区分「保持旧值 / 设置新值 / 清除已存凭据」
///
/// wire 格式（untagged，向后兼容旧前端）：
/// - 缺失或 null → Keep：保持旧引用不变
/// - 字符串 → Set：写入安全存储并引用（空串等价于 Keep，兼容「留空则保持」）
/// - `{"clear": true}` → Clear：引用置 None，commit 成功后由保存流程尽力删除已存凭据
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum CredentialInput {
    /// 显式清除已存凭据；clear 字段本身不参与语义判断
    Clear { clear: bool },
    /// 明文凭据；空串等价于 Keep
    Set(String),
}
