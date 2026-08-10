use crate::errors::app_error::{AppError, AppErrorInfo};
use log::LevelFilter;

/** 解析前端传入的日志等级。 */
fn parse_log_level(level: &str) -> Option<LevelFilter> {
    match level {
        "error" => Some(LevelFilter::Error),
        "warn" => Some(LevelFilter::Warn),
        "info" => Some(LevelFilter::Info),
        "debug" => Some(LevelFilter::Debug),
        "trace" => Some(LevelFilter::Trace),
        _ => None,
    }
}

/// 设置运行中日志器的最大输出等级。
#[tauri::command]
pub fn set_log_level(level: String) -> Result<(), AppErrorInfo> {
    let level = parse_log_level(&level).ok_or_else(|| {
        AppErrorInfo::from(AppError::InvalidHostConfig("Invalid log level".to_string()))
    })?;
    log::set_max_level(level);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::parse_log_level;
    use log::LevelFilter;

    /// 仅接受前端下拉框提供的稳定日志等级。
    #[test]
    fn parses_supported_log_levels_and_rejects_invalid_values() {
        assert_eq!(parse_log_level("debug"), Some(LevelFilter::Debug));
        assert_eq!(parse_log_level("verbose"), None);
    }
}
