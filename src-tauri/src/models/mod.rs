pub mod host;
pub mod host_identity;
pub mod monitor;
pub mod session;
pub mod sftp;

#[cfg(test)]
mod tests {
    use super::host::{AuthType, HostConfig};
    use super::monitor::{
        MonitorSnapshot, NetworkInterface, NetworkSnapshot, TaskInfo, TaskStatus,
    };
    use super::session::{SessionInfo, SessionStatus, TerminalDataEvent};
    use super::sftp::{SftpProgressEvent, SftpTaskStatus, TransferTask, TransferType};
    use serde_json::json;

    /// 验证所有公开 JSON 模型统一输出 camelCase 字段。
    #[test]
    fn public_models_serialize_as_camel_case() {
        let values = [
            serde_json::to_value(SessionInfo {
                session_id: "session-1".into(),
                host_id: "host-1".into(),
                host: "127.0.0.1".into(),
                port: 22,
                username: "root".into(),
                status: SessionStatus::Connected,
                created_at: 1,
            })
            .unwrap(),
            serde_json::to_value(MonitorSnapshot {
                session_id: "session-1".into(),
                timestamp: 1,
                cpu_usage: Some(1.0),
                memory_usage: Some(2.0),
                disk_usage: Some(3.0),
                disk_available_bytes: Some(4),
                disk_total_bytes: Some(5),
                network: NetworkSnapshot {
                    available: true,
                    interfaces: vec![NetworkInterface {
                        name: "eth0".into(),
                        receive_bytes_per_second: Some(4),
                        transmit_bytes_per_second: None,
                    }],
                },
            })
            .unwrap(),
            serde_json::to_value(TaskInfo {
                task_id: "task-1".into(),
                task_type: "monitor".into(),
                session_id: Some("session-1".into()),
                status: TaskStatus::Running,
                created_at: 1,
            })
            .unwrap(),
            serde_json::to_value(TransferTask {
                task_id: "task-1".into(),
                session_id: "session-1".into(),
                transfer_type: TransferType::Download,
                remote_path: "/a".into(),
                local_path: "/b".into(),
                file_name: "a".into(),
                total_bytes: 10,
                transferred_bytes: 5,
                speed_bps: 2,
                status: SftpTaskStatus::Running,
                error: None,
                created_at: 1,
            })
            .unwrap(),
            serde_json::to_value(SftpProgressEvent {
                task_id: "task-1".into(),
                session_id: "session-1".into(),
                transferred_bytes: 5,
                total_bytes: 10,
                speed_bps: 2,
            })
            .unwrap(),
            serde_json::to_value(TerminalDataEvent {
                session_id: "session-1".into(),
                data: "ok".into(),
            })
            .unwrap(),
        ];

        for value in values {
            let object = value.as_object().unwrap();
            assert!(
                object.keys().all(|key| !key.contains('_')),
                "发现 snake_case 字段: {object:?}"
            );
        }

        let monitor = serde_json::to_value(MonitorSnapshot {
            session_id: "session-1".into(),
            timestamp: 1,
            cpu_usage: Some(1.0),
            memory_usage: Some(2.0),
            disk_usage: Some(3.0),
            disk_available_bytes: Some(4),
            disk_total_bytes: Some(5),
            network: NetworkSnapshot {
                available: true,
                interfaces: vec![NetworkInterface {
                    name: "eth0".into(),
                    receive_bytes_per_second: Some(4),
                    transmit_bytes_per_second: None,
                }],
            },
        })
        .unwrap();
        let interface = &monitor["network"]["interfaces"][0];
        assert_eq!(interface["receiveBytesPerSecond"], 4);
        assert!(interface.get("receive_bytes_per_second").is_none());
    }

    /// 验证升级后仍可读取旧版本写入的 snake_case 主机配置。
    #[test]
    fn host_config_accepts_legacy_snake_case() {
        let host: HostConfig = serde_json::from_value(json!({
            "id": "host-1", "name": "prod", "host": "127.0.0.1", "port": 22, "username": "root",
            "auth_type": "Password", "password_ref": "password-key", "private_key_path": null,
            "passphrase_ref": null, "remark": null
        }))
        .unwrap();

        assert_eq!(host.auth_type, AuthType::Password);
        assert_eq!(host.password_ref.as_deref(), Some("password-key"));
    }

    /// 验证旧版本写入的主机配置缺少 group 字段时仍可读取，并补默认空串（"未分组"）。
    #[test]
    fn host_config_defaults_missing_group_field() {
        let host: HostConfig = serde_json::from_value(json!({
            "id": "host-1", "name": "prod", "host": "127.0.0.1", "port": 22, "username": "root",
            "authType": "Password", "passwordRef": null, "privateKeyPath": null,
            "passphraseRef": null, "remark": null
        }))
        .unwrap();

        assert_eq!(host.group, "");
    }
}
