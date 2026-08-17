#[cfg(test)]
mod tests {
    use crate::errors::app_error::{AppError, AppErrorInfo, ErrorDetail};

    /// SSH 协议错误只保存稳定文本，不向所属 module 外泄漏 ssh2 错误类型。
    #[test]
    fn ssh_protocol_error_contains_only_stable_text() {
        let error = AppError::SshProtocolError("channel failed".to_string().into());

        assert_eq!(error.to_string(), "SSH 协议错误: channel failed");
    }

    /// 日志错误代码始终使用稳定英文标识。
    #[test]
    fn app_error_code_is_english_and_stable() {
        assert_eq!(
            AppError::CredentialNotFound(ErrorDetail::msg("主机密码", Vec::new())).code(),
            "CredentialNotFound"
        );
    }

    /// IPC 错误使用稳定代码与 camelCase detail，不携带已本地化 UI 文案。
    #[test]
    fn app_error_info_serializes_raw_detail_as_plain_string() {
        let value = serde_json::to_value(AppErrorInfo::from(AppError::AuthenticationError(
            "denied".to_string().into(),
        )))
        .expect("错误 payload 应序列化");
        assert_eq!(
            value,
            serde_json::json!({ "code": "AuthenticationError", "detail": "denied" })
        );
    }

    /// Msg 详情序列化为 detailKey + detailParams，detail 字段缺席；
    /// 前端按当前语言翻译模板，参数按占位顺序替换。
    #[test]
    fn app_error_info_serializes_msg_detail_as_key_and_params() {
        let value = serde_json::to_value(AppErrorInfo::from(AppError::StorageError(
            ErrorDetail::msg(
                "读取主机配置文件失败: {0}",
                vec!["permission denied".to_string()],
            ),
        )))
        .expect("错误 payload 应序列化");
        assert_eq!(
            value,
            serde_json::json!({
                "code": "StorageError",
                "detailKey": "读取主机配置文件失败: {0}",
                "detailParams": ["permission denied"]
            })
        );
    }

    /// IPC 边界必须移除凭据、口令和私钥内容；后端日志仍可保留完整内部诊断。
    #[test]
    fn app_error_info_redacts_sensitive_diagnostics_before_ipc() {
        let passphrase = "correct-horse-battery-staple";
        let private_key =
            "-----BEGIN OPENSSH PRIVATE KEY-----\nvery-secret-key-material\n-----END OPENSSH PRIVATE KEY-----";
        let raw = AppErrorInfo::from(AppError::AuthenticationError(
            format!("authentication failed: password={passphrase}; key={private_key}").into(),
        ));
        let structured = AppErrorInfo::from(AppError::SecureStoreError(ErrorDetail::msg(
            "安全存储读取失败: {0}",
            vec![format!("passphrase: {passphrase}")],
        )));

        let raw_json = serde_json::to_string(&raw).expect("Raw IPC payload 应序列化");
        let structured_json = serde_json::to_string(&structured).expect("Msg IPC payload 应序列化");
        for secret in [passphrase, "very-secret-key-material"] {
            assert!(
                !raw_json.contains(secret),
                "Raw IPC payload 不得泄露敏感值: {raw_json}"
            );
            assert!(
                !structured_json.contains(secret),
                "Msg IPC payload 不得泄露敏感值: {structured_json}"
            );
        }
        assert_eq!(
            raw.detail.as_deref(),
            Some("authentication failed: password=[REDACTED]; key=[REDACTED]")
        );
        assert_eq!(
            structured.detail_params,
            Some(vec!["passphrase: [REDACTED]".to_string()])
        );
    }

    /// Msg 模板占位按序替换；无占位的额外参数（with_appended_detail）以「；」连接。
    #[test]
    fn msg_detail_renders_template_placeholders_and_appended_params() {
        let detail = ErrorDetail::msg(
            "endpoint {0} 的信任记录清理失败: {1}",
            vec!["10.0.0.8:22".to_string(), "write denied".to_string()],
        );
        assert_eq!(
            detail.to_string(),
            "endpoint 10.0.0.8:22 的信任记录清理失败: write denied"
        );

        let appended = AppError::StorageError(detail)
            .with_appended_detail("cleanup: /tmp/.f.part")
            .to_string();
        assert_eq!(
            appended,
            "存储错误: endpoint 10.0.0.8:22 的信任记录清理失败: write denied；cleanup: /tmp/.f.part"
        );
    }

    /// 下载冲突/占用/发布错误使用稳定英文代码，detail 携带目标路径。
    #[test]
    fn download_conflict_errors_have_stable_codes() {
        assert_eq!(
            AppError::SftpTargetExists("/tmp/a.txt".to_string().into()).code(),
            "SftpTargetExists"
        );
        assert_eq!(
            AppError::SftpTargetBusy("/tmp/a.txt".to_string().into()).code(),
            "SftpTargetBusy"
        );
        assert_eq!(
            AppError::SftpPublishError("/tmp/a.txt".to_string().into()).code(),
            "SftpPublishError"
        );
    }

    /// 主机身份错误使用稳定英文代码，供前端按 code 区分拒绝/取消/请求不存在。
    #[test]
    fn host_identity_errors_have_stable_codes() {
        assert_eq!(
            AppError::HostKeyRejected("10.0.0.8:22".to_string().into()).code(),
            "HostKeyRejected"
        );
        assert_eq!(
            AppError::HostKeyChallengeNotFound("challenge-1".to_string().into()).code(),
            "HostKeyChallengeNotFound"
        );
        assert_eq!(
            AppError::HostKeyVerificationCancelled("session-1".to_string().into()).code(),
            "HostKeyVerificationCancelled"
        );
    }

    /// 信任存储与保存失败使用稳定英文代码，供前端区分 fail-closed 与保存失败。
    #[test]
    fn trust_store_errors_have_stable_codes() {
        assert_eq!(
            AppError::TrustStoreError(ErrorDetail::msg("known_hosts 解析失败", Vec::new())).code(),
            "TrustStoreError"
        );
        assert_eq!(
            AppError::HostKeySaveFailed("write denied".to_string().into()).code(),
            "HostKeySaveFailed"
        );
    }

    /// HostConfig 生命周期清理失败使用稳定英文代码，IPC payload 结构化且
    /// 携带 endpoint 诊断：管理动作不得被静默报告为成功。
    #[test]
    fn host_trust_cleanup_error_has_stable_code_and_payload() {
        let error = AppError::HostTrustCleanupFailed(ErrorDetail::msg(
            "endpoint {0} 的信任记录清理失败: {1}",
            vec!["10.0.0.8:22".to_string(), "write denied".to_string()],
        ));
        assert_eq!(error.code(), "HostTrustCleanupFailed");
        let value = serde_json::to_value(AppErrorInfo::from(error)).unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "code": "HostTrustCleanupFailed",
                "detailKey": "endpoint {0} 的信任记录清理失败: {1}",
                "detailParams": ["10.0.0.8:22", "write denied"]
            })
        );
    }

    /// with_appended_detail 保持原错误代码不变，并把补充说明追加到详情。
    #[test]
    fn appended_detail_preserves_code_and_appends_text() {
        let error = AppError::SftpReadError("remote read reset".to_string().into())
            .with_appended_detail("cleanup failed: /tmp/.f.part (permission denied)");
        assert_eq!(error.code(), "SftpReadError");
        assert!(error.to_string().contains("remote read reset"));
        assert!(
            error
                .to_string()
                .contains("cleanup failed: /tmp/.f.part (permission denied)")
        );
    }
}
