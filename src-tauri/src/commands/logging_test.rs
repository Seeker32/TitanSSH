#[cfg(test)]
mod tests {
    use crate::commands::logging::parse_log_level;
    use log::LevelFilter;

    /// 仅接受前端下拉框提供的稳定日志等级。
    #[test]
    fn parses_supported_log_levels_and_rejects_invalid_values() {
        assert_eq!(parse_log_level("debug"), Some(LevelFilter::Debug));
        assert_eq!(parse_log_level("verbose"), None);
    }
}
