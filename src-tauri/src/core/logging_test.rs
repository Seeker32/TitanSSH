#[cfg(test)]
mod tests {
    use crate::core::logging::{
        LOG_FILE_NAME, LOG_MAX_BYTES, LOG_VIEW_MAX_LINES, LogStore, ensure_log_file, format_entry,
        install_logger,
    };
    use log::Level;
    use std::fs::{self, OpenOptions};
    use std::io::Write;
    use tempfile::tempdir;

    /// 格式化输出固定单行：时间戳 + 等级 + 目标 + 消息。
    #[test]
    fn format_entry_produces_stable_single_line() {
        let line = format_entry(
            Level::Info,
            "core::logging",
            "连接已建立",
            "2025-06-01 14:30:00.123",
        );
        assert_eq!(
            line,
            "2025-06-01 14:30:00.123 [INFO] core::logging: 连接已建立"
        );
    }

    /// 行数低于查看上限时按顺序完整返回。
    #[test]
    fn read_recent_returns_full_content_when_under_limit() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(LOG_FILE_NAME);
        fs::write(&path, "line-1\nline-2\nline-3\n").unwrap();

        let lines = LogStore::from_file_path(path).read_recent().unwrap();
        assert_eq!(lines, vec!["line-1", "line-2", "line-3"]);
    }

    /// 超过上限只返回最后 LOG_VIEW_MAX_LINES 行，最新在末尾。
    #[test]
    fn read_recent_returns_only_last_view_limit_lines_newest_last() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(LOG_FILE_NAME);
        let content: Vec<String> = (0..=LOG_VIEW_MAX_LINES)
            .map(|i| format!("line-{i}"))
            .collect();
        fs::write(&path, content.join("\n")).unwrap();

        let lines = LogStore::from_file_path(path).read_recent().unwrap();
        assert_eq!(lines.len(), LOG_VIEW_MAX_LINES);
        assert_eq!(lines.first().unwrap(), "line-1", "最旧的一行应被丢弃");
        assert_eq!(lines.last().unwrap(), &format!("line-{LOG_VIEW_MAX_LINES}"));
    }

    /// 日志文件不存在时返回空列表（对应首次运行/尚未产生日志）。
    #[test]
    fn read_recent_returns_empty_when_file_missing() {
        let dir = tempdir().unwrap();
        let lines = LogStore::from_file_path(dir.path().join(LOG_FILE_NAME))
            .read_recent()
            .unwrap();
        assert!(lines.is_empty());
    }

    /// 导出复制内容并可覆盖已存在的目标文件。
    #[test]
    fn export_to_copies_content_and_overwrites_existing_target() {
        let dir = tempdir().unwrap();
        let source = dir.path().join(LOG_FILE_NAME);
        fs::write(&source, "debug-1\ndebug-2\n").unwrap();
        let target = dir.path().join("exported.log");
        fs::write(&target, "old").unwrap();

        LogStore::from_file_path(source).export_to(&target).unwrap();
        assert_eq!(fs::read_to_string(&target).unwrap(), "debug-1\ndebug-2\n");
    }

    /// 源日志文件缺失时导出返回稳定 IoError 错误码。
    #[test]
    fn export_to_fails_with_io_error_when_source_missing() {
        let dir = tempdir().unwrap();
        let error = LogStore::from_file_path(dir.path().join(LOG_FILE_NAME))
            .export_to(&dir.path().join("exported.log"))
            .unwrap_err();
        assert_eq!(error.code(), "IoError");
    }

    /// 小文件追加保留、超限文件截断后重新记录。
    #[test]
    fn ensure_log_file_creates_appends_and_truncates_oversized_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(LOG_FILE_NAME);

        let mut file = ensure_log_file(&path).unwrap();
        writeln!(file, "first").unwrap();
        drop(file);
        let mut file = ensure_log_file(&path).unwrap();
        writeln!(file, "second").unwrap();
        drop(file);
        assert_eq!(fs::read_to_string(&path).unwrap(), "first\nsecond\n");

        // set_len 伪造超大文件（稀疏文件，磁盘占用小），验证启动截断
        let file = OpenOptions::new().write(true).open(&path).unwrap();
        file.set_len(LOG_MAX_BYTES + 1).unwrap();
        drop(file);
        let mut file = ensure_log_file(&path).unwrap();
        writeln!(file, "fresh").unwrap();
        drop(file);
        assert_eq!(fs::read_to_string(&path).unwrap(), "fresh\n");
    }

    /// 日志目录不存在时递归创建。
    #[test]
    fn ensure_log_file_creates_missing_parent_directories() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("nested").join("logs").join(LOG_FILE_NAME);
        ensure_log_file(&path).unwrap();
        assert!(path.exists());
    }

    /// 重复安装不 panic（第二次 set_logger 被忽略）；路径缺失时退化为仅 stderr。
    #[test]
    fn install_logger_is_idempotent_and_falls_back_to_stderr_only() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(LOG_FILE_NAME);
        install_logger(Some(&path));
        install_logger(Some(&path));
        install_logger(None);
    }
}
