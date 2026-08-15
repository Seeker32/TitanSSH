#[cfg(test)]
mod tests {
    use crate::errors::app_error::{AppError, AppErrorInfo};

    /// SSH 协议错误只保存稳定文本，不向所属 module 外泄漏 ssh2 错误类型。
    #[test]
    fn ssh_protocol_error_contains_only_stable_text() {
        let error = AppError::SshProtocolError("channel failed".to_string());

        assert_eq!(error.to_string(), "SSH 协议错误: channel failed");
    }

    /// 日志错误代码始终使用稳定英文标识。
    #[test]
    fn app_error_code_is_english_and_stable() {
        assert_eq!(
            AppError::CredentialNotFound("主机密码".to_string()).code(),
            "CredentialNotFound"
        );
    }

    /// IPC 错误使用稳定代码与 camelCase detail，不携带已本地化 UI 文案。
    #[test]
    fn app_error_info_serializes_as_structured_payload() {
        let value = serde_json::to_value(AppErrorInfo::from(AppError::AuthenticationError(
            "denied".to_string(),
        )))
        .expect("错误 payload 应序列化");
        assert_eq!(
            value,
            serde_json::json!({ "code": "AuthenticationError", "detail": "denied" })
        );
    }

    /// 下载冲突/占用/发布错误使用稳定英文代码，detail 携带目标路径。
    #[test]
    fn download_conflict_errors_have_stable_codes() {
        assert_eq!(
            AppError::SftpTargetExists("/tmp/a.txt".to_string()).code(),
            "SftpTargetExists"
        );
        assert_eq!(
            AppError::SftpTargetBusy("/tmp/a.txt".to_string()).code(),
            "SftpTargetBusy"
        );
        assert_eq!(
            AppError::SftpPublishError("/tmp/a.txt".to_string()).code(),
            "SftpPublishError"
        );
    }

    /// 主机身份错误使用稳定英文代码，供前端按 code 区分拒绝/取消/请求不存在。
    #[test]
    fn host_identity_errors_have_stable_codes() {
        assert_eq!(
            AppError::HostKeyRejected("10.0.0.8:22".to_string()).code(),
            "HostKeyRejected"
        );
        assert_eq!(
            AppError::HostKeyChallengeNotFound("challenge-1".to_string()).code(),
            "HostKeyChallengeNotFound"
        );
        assert_eq!(
            AppError::HostKeyVerificationCancelled("session-1".to_string()).code(),
            "HostKeyVerificationCancelled"
        );
    }

    /// 信任存储与保存失败使用稳定英文代码，供前端区分 fail-closed 与保存失败。
    #[test]
    fn trust_store_errors_have_stable_codes() {
        assert_eq!(
            AppError::TrustStoreError("known_hosts 解析失败".to_string()).code(),
            "TrustStoreError"
        );
        assert_eq!(
            AppError::HostKeySaveFailed("write denied".to_string()).code(),
            "HostKeySaveFailed"
        );
    }

    /// HostConfig 生命周期清理失败使用稳定英文代码，IPC payload 结构化且
    /// 携带 endpoint 诊断：管理动作不得被静默报告为成功。
    #[test]
    fn host_trust_cleanup_error_has_stable_code_and_payload() {
        let error = AppError::HostTrustCleanupFailed(
            "endpoint 10.0.0.8:22 的信任记录清理失败: write denied".to_string(),
        );
        assert_eq!(error.code(), "HostTrustCleanupFailed");
        let value = serde_json::to_value(AppErrorInfo::from(error)).unwrap();
        assert_eq!(
            value,
            serde_json::json!({ "code": "HostTrustCleanupFailed", "detail": "endpoint 10.0.0.8:22 的信任记录清理失败: write denied" })
        );
    }

    /// with_appended_detail 保持原错误代码不变，并把补充说明追加到 detail。
    #[test]
    fn appended_detail_preserves_code_and_appends_text() {
        let error = AppError::SftpReadError("remote read reset".to_string())
            .with_appended_detail("清理临时文件失败: /tmp/.f.part (permission denied)");
        assert_eq!(error.code(), "SftpReadError");
        assert!(error.to_string().contains("remote read reset"));
        assert!(error.to_string().contains("清理临时文件失败: /tmp/.f.part"));
    }
}
