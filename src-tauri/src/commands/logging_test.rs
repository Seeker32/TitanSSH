#[cfg(test)]
mod tests {
    use crate::commands::logging::{parse_log_level, resolve_export_target};
    use crate::errors::app_error::AppError;
    use log::LevelFilter;
    use std::path::PathBuf;
    use tauri_plugin_dialog::FilePath;

    /// 仅接受前端下拉框提供的稳定日志等级。
    #[test]
    fn parses_supported_log_levels_and_rejects_invalid_values() {
        assert_eq!(parse_log_level("debug"), Some(LevelFilter::Debug));
        assert_eq!(parse_log_level("verbose"), None);
    }

    /// 取消（None）→ Ok(None)；本地路径 → Some；无法解析为本地路径的 URL 必须报错，
    /// 绝不与取消混淆（否则用户会误以为导出成功）。
    #[test]
    fn resolves_export_target_distinguishes_cancel_from_path_resolution_failure() {
        // 用户取消：不是错误
        assert!(matches!(resolve_export_target(None), Ok(None)));

        // 普通本地路径
        let path = PathBuf::from("/tmp/titanssh-export.log");
        let picked = FilePath::from(path.clone());
        assert_eq!(resolve_export_target(Some(picked)).unwrap(), Some(path));

        // 无法落地的 URL（如云盘虚拟文件）：必须返回错误；专用 code 供前端按语言
        // 本地化摘要，detail 只保留底层诊断（英文机器信息），绝不把中文文案塞进 detail
        let remote = FilePath::Url(url::Url::parse("https://example.com/remote.log").unwrap());
        let error = resolve_export_target(Some(remote)).unwrap_err();
        assert!(matches!(error, AppError::LogExportPathResolveFailed(_)));
        assert_eq!(error.code(), "LogExportPathResolveFailed");
    }
}
