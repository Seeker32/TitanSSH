#[cfg(test)]
mod tests {
    use crate::models::sftp::ConflictStrategy;

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
