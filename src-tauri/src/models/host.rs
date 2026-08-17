use serde::{Deserialize, Serialize};
use std::fmt;

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
///
/// 仅实现 Deserialize：此类型是 Tauri 的单向入站请求，绝不允许被序列化为日志、
/// 事件或持久化 payload。Debug 也必须始终隐藏密码与私钥口令。
#[derive(Clone, Deserialize)]
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

/// SaveHostRequest 的调试输出仅保留非敏感配置，避免诊断路径泄露明文凭据。
impl fmt::Debug for SaveHostRequest {
    /// 渲染请求的非敏感字段；password 与 passphrase 始终显示为脱敏占位符。
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SaveHostRequest")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("host", &self.host)
            .field("port", &self.port)
            .field("username", &self.username)
            .field("auth_type", &self.auth_type)
            .field("password", &RedactedCredentialInput(&self.password))
            .field("private_key_path", &self.private_key_path)
            .field("passphrase", &RedactedCredentialInput(&self.passphrase))
            .field("remark", &self.remark)
            .field("group", &self.group)
            .finish()
    }
}

/// Debug 的凭据占位包装器；保留是否提供凭据的信息，但绝不渲染其具体值。
struct RedactedCredentialInput<'a>(&'a Option<CredentialInput>);

impl fmt::Debug for RedactedCredentialInput<'_> {
    /// 将任意已提供的凭据显示为固定占位符，避免调试、日志和错误富化泄露明文。
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0.is_some() {
            formatter.write_str("Some([REDACTED])")
        } else {
            formatter.write_str("None")
        }
    }
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

#[cfg(test)]
mod tests {
    use super::{AuthType, CredentialInput, SaveHostRequest};

    /// SaveHostRequest 的调试输出可用于安全诊断，但绝不包含提交中的明文凭据。
    #[test]
    fn save_host_request_debug_redacts_password_and_passphrase() {
        let request = SaveHostRequest {
            id: "host-1".to_string(),
            name: "production".to_string(),
            host: "10.0.0.8".to_string(),
            port: 22,
            username: "ops".to_string(),
            auth_type: AuthType::PrivateKey,
            password: Some(CredentialInput::Set("password-should-not-appear".to_string())),
            private_key_path: Some("/keys/production".to_string()),
            passphrase: Some(CredentialInput::Set("passphrase-should-not-appear".to_string())),
            remark: None,
            group: "production".to_string(),
        };

        let rendered = format!("{request:?}");

        assert!(rendered.contains("id: \"host-1\""));
        assert!(rendered.contains("password: Some([REDACTED])"));
        assert!(rendered.contains("passphrase: Some([REDACTED])"));
        assert!(!rendered.contains("password-should-not-appear"));
        assert!(!rendered.contains("passphrase-should-not-appear"));
    }
}
