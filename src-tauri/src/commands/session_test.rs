#[cfg(test)]
mod tests {
    use crate::commands::session::run_host_lookup;
    use crate::core::host_identity::HostIdentityService;
    use crate::core::monitor_service::MonitorService;
    use crate::core::session_manager::SessionManager;
    use crate::core::sftp_service::SftpService;
    use crate::errors::app_error::{AppError, AppErrorInfo};
    use crate::models::host::{AuthType, HostConfig};
    use std::sync::mpsc;
    use std::time::Duration;
    use tauri::ipc::{CallbackFn, InvokeBody};
    use tauri::test::{get_ipc_response, mock_builder, mock_context, noop_assets};
    use tauri::webview::InvokeRequest;

    /// 构造不含明文凭据的测试主机。
    fn make_host(id: &str) -> HostConfig {
        HostConfig {
            id: id.to_string(),
            name: "test".to_string(),
            host: "127.0.0.1".to_string(),
            port: 22,
            username: "root".to_string(),
            auth_type: AuthType::Password,
            password_ref: Some("ref".to_string()),
            private_key_path: None,
            passphrase_ref: None,
            remark: None,
            group: String::new(),
        }
    }

    /// 构造带 session header 的 raw Tauri IPC 请求。
    fn raw_request(session_id: &str, body: Vec<u8>) -> InvokeRequest {
        let mut request = InvokeRequest {
            cmd: "write_terminal".to_string(),
            callback: CallbackFn(0),
            error: CallbackFn(1),
            url: "http://tauri.localhost".parse().unwrap(),
            body: InvokeBody::Raw(body),
            headers: Default::default(),
            invoke_key: tauri::test::INVOKE_KEY.to_string(),
        };
        request.headers.insert(
            "x-titanssh-session-id",
            session_id.parse().expect("session header 应为合法文本"),
        );
        request
    }

    /// raw IPC payload 中的任意字节必须无损抵达 TerminalCommand。
    #[test]
    fn write_terminal_accepts_raw_bytes() {
        let manager = SessionManager::new(
            MonitorService::new(),
            SftpService::new(),
            HostIdentityService::new(),
        );
        let session_id = "session-raw-input";
        let receiver =
            manager.insert_session_for_test_with_receiver(session_id, make_host("host-1"));
        let app = mock_builder()
            .manage(manager)
            .invoke_handler(tauri::generate_handler![
                crate::commands::session::write_terminal
            ])
            .build(mock_context(noop_assets()))
            .expect("mock app 应构造成功");
        let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .expect("mock webview 应构造成功");
        let input = vec![0x00, 0xff, 0x1b, b'[', b'A'];

        let response = get_ipc_response(&webview, raw_request(session_id, input.clone()))
            .expect("raw 终端输入应成功路由");
        let value: serde_json::Value = response.deserialize().expect("成功响应应为 JSON null");
        assert_eq!(value, serde_json::Value::Null);

        match receiver.recv().expect("应收到终端写入命令") {
            crate::core::terminal_service::TerminalCommand::Write(data) => {
                assert_eq!(data, input)
            }
            _ => panic!("期望 Write 命令"),
        }

        let mut invalid_request = raw_request(session_id, Vec::new());
        invalid_request.body = InvokeBody::Json(serde_json::json!({}));
        let response = get_ipc_response(&webview, invalid_request).expect_err("JSON 输入应被拒绝");
        let error: AppErrorInfo = serde_json::from_value(response).expect("错误应为结构化 payload");
        assert_eq!(error.code, "InvalidTerminalInput");
    }

    /// 回归：打开会话前的主机配置读取可能等待磁盘，必须在阻塞线程池执行，
    /// 不能占用调用线程（真实应用中的 Tauri 主线程）。
    #[test]
    fn host_lookup_for_open_session_executes_off_caller_thread() {
        let (started_tx, started_rx) = mpsc::channel::<std::thread::ThreadId>();
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let caller_id = std::thread::current().id();

        let task = tauri::async_runtime::spawn(run_host_lookup(move || {
            started_tx
                .send(std::thread::current().id())
                .expect("查询线程 ID 应送达");
            release_rx.recv().expect("release 信号应送达");
            Ok::<i32, AppError>(7)
        }));

        let worker_id = started_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("主机查询任务应已开始");
        assert_ne!(worker_id, caller_id, "主机查询不得占用调用线程");

        release_tx
            .send(())
            .expect("调用线程必须不被阻塞，可立即解除查询等待");
        let result = tauri::async_runtime::block_on(task)
            .expect("任务应正常完成")
            .expect("主机查询应成功");
        assert_eq!(result, 7);
    }
}
