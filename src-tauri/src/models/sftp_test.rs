#[cfg(test)]
mod tests {
    use crate::models::sftp::{ConflictStrategy, RemoteEntry};

    /// 构造远程条目，聚焦服务端路径/名称信任边界测试。
    fn remote_entry(name: &str, path: &str) -> RemoteEntry {
        RemoteEntry {
            name: name.to_string(),
            path: path.to_string(),
            is_dir: false,
            size: 1,
            modified_at: 0,
            permissions: "rw-r--r--".to_string(),
        }
    }

    /// 仅接受规范绝对 POSIX 路径与匹配的单段文件名，避免服务端输入参与本地路径派生。
    #[test]
    fn remote_entry_file_name_accepts_only_canonical_safe_entries() {
        let valid = remote_entry("syslog", "/var/log/syslog");
        assert!(valid.is_valid_entry());
        assert_eq!(valid.file_name(), Some("syslog"));

        for invalid in [
            remote_entry("syslog", "var/log/syslog"),
            remote_entry("syslog", "/var//log/syslog"),
            remote_entry("syslog", "/var/./log/syslog"),
            remote_entry("syslog", "/var/log/../syslog"),
            remote_entry("../syslog", "/var/log/syslog"),
            remote_entry("syslog\\backup", "/var/log/syslog"),
            remote_entry("other", "/var/log/syslog"),
            remote_entry("syslog", "/var/log/syslog\\backup"),
        ] {
            assert!(!invalid.is_valid_entry(), "畸形条目不得通过: {invalid:?}");
            assert_eq!(invalid.file_name(), None);
        }
    }

    /// 冲突策略默认值为 Reject：未显式指定时绝不覆盖本地文件。
    #[test]
    fn conflict_strategy_defaults_to_reject() {
        assert_eq!(ConflictStrategy::default(), ConflictStrategy::Reject);
    }

    /// IPC 载荷与 TransferType/TaskStatus 同约定：PascalCase 字符串往返。
    #[test]
    fn conflict_strategy_roundtrips_pascal_case() {
        let value: ConflictStrategy = serde_json::from_str("\"Overwrite\"").unwrap();
        assert_eq!(value, ConflictStrategy::Overwrite);
        let value: ConflictStrategy = serde_json::from_str("\"Reject\"").unwrap();
        assert_eq!(value, ConflictStrategy::Reject);
        assert_eq!(
            serde_json::to_string(&ConflictStrategy::Overwrite).unwrap(),
            "\"Overwrite\""
        );
    }
}
