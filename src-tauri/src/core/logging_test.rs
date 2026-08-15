#[cfg(test)]
mod tests {
    use crate::core::logging::{
        LOG_FILE_NAME, LOG_MAX_BYTES, LOG_VIEW_MAX_LINES, LogStore, ensure_log_file, format_entry,
        install_logger, logger_install_recorded,
    };
    use log::Level;
    use std::fs::{self, OpenOptions};
    use std::io::Write;
    use std::sync::Mutex;
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

    /// seek 尾读：首行超过尾读窗口时整行被丢弃，只返回窗口内的完整行。
    /// （全文件读取会把超长首行也返回，本测试守护 seek 实现。）
    #[test]
    fn read_recent_seeks_tail_and_skips_partial_first_line() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(LOG_FILE_NAME);
        let giant = "x".repeat(200_000);
        fs::write(&path, format!("{giant}\ntail-1\ntail-2\ntail-3\n")).unwrap();

        let lines = LogStore::from_file_path(path).read_recent().unwrap();
        assert_eq!(lines, vec!["tail-1", "tail-2", "tail-3"]);
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

    /// 外部日志器占位：首次安装失败必须被记录（触发 stderr 降级诊断），
    /// 重复安装保持静默（幂等语义，不重复报错）。
    #[test]
    fn install_logger_foreign_occupation_is_recorded_and_reinstall_is_silent() {
        struct ForeignLogger;
        impl log::Log for ForeignLogger {
            fn enabled(&self, _: &log::Metadata) -> bool {
                false
            }
            fn log(&self, _: &log::Record) {}
            fn flush(&self) {}
        }
        // 模拟插件抢先安装的其他日志器（测试进程内无其他测试占用全局槽位）
        log::set_boxed_logger(Box::new(ForeignLogger)).expect("日志器槽位应空闲");

        let dir = tempdir().unwrap();
        let path = dir.path().join(LOG_FILE_NAME);
        install_logger(Some(&path)); // 首次安装失败：记录并留 stderr 诊断
        assert!(
            logger_install_recorded(),
            "首次安装失败必须被记录，否则降级诊断不会输出"
        );
        install_logger(Some(&path)); // 已记录：静默，不 panic
        install_logger(None);
    }

    /// 直接构造 Logger 验证文件落盘与 flush（不依赖全局日志器槽位，
    /// 避免与其他测试竞争全局 log facade 状态）。
    #[test]
    fn logger_writes_records_to_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(LOG_FILE_NAME);
        let file = ensure_log_file(&path).unwrap();
        let logger = crate::core::logging::Logger {
            stderr_logger: env_logger::Builder::new()
                .filter_level(log::LevelFilter::Trace)
                .build(),
            file: Some(Mutex::new(file)),
        };
        let metadata = log::MetadataBuilder::new()
            .level(Level::Info)
            .target("core::logging")
            .build();
        let record = log::Record::builder()
            .metadata(metadata)
            .args(format_args!("连接已建立"))
            .build();
        log::Log::log(&logger, &record);
        log::Log::flush(&logger);
        drop(logger);

        let content = fs::read_to_string(&path).unwrap();
        assert!(
            content.contains("[INFO] core::logging: 连接已建立"),
            "日志应写入文件，实际内容: {content}"
        );
    }
}
