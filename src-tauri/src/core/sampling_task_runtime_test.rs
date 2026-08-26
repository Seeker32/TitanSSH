use super::*;
use crate::errors::app_error::AppError;
use crate::models::host::{AuthType, HostConfig};
use crate::models::monitor::TaskStatus;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use tauri::Listener;
use tauri::test::mock_app;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
struct Snapshot {
    value: u32,
}

fn spec() -> SamplingTaskSpec {
    SamplingTaskSpec {
        task_type: "test",
        snapshot_event: "test:snapshot",
        error_code: "TestError",
        worker_panic_detail_key: "测试工作线程异常退出: {0}",
        snapshot_emit_detail_key: "测试快照推送失败: {0}",
    }
}

fn host() -> HostConfig {
    HostConfig {
        id: "host-1".to_string(),
        name: "test".to_string(),
        host: "127.0.0.1".to_string(),
        port: 22,
        username: "root".to_string(),
        auth_type: AuthType::PrivateKey,
        password_ref: None,
        private_key_path: Some("/tmp/test-key".to_string()),
        passphrase_ref: None,
        remark: None,
        group: String::new(),
    }
}

fn statuses(app: &tauri::AppHandle<tauri::test::MockRuntime>) -> Arc<Mutex<Vec<TaskStatus>>> {
    let statuses = Arc::new(Mutex::new(Vec::new()));
    let captured = statuses.clone();
    app.listen("task:status", move |event| {
        let payload: serde_json::Value = serde_json::from_str(event.payload()).unwrap();
        captured
            .lock()
            .unwrap()
            .push(serde_json::from_value(payload["status"].clone()).unwrap());
    });
    statuses
}

fn wait_until(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(2);
    while !condition() {
        assert!(Instant::now() < deadline, "测试等待超时");
        thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn lifecycle_publishes_pending_running_snapshot_done() {
    let app = mock_app();
    let statuses = statuses(app.handle());
    let runtime = SamplingTaskRuntime::<Snapshot>::new(spec());

    runtime
        .start(
            app.handle().clone(),
            "session-1".to_string(),
            host(),
            |input, sink| {
                assert_eq!(input.session_id, "session-1");
                sink.publish(Snapshot { value: 7 });
            },
        )
        .unwrap();

    wait_until(|| statuses.lock().unwrap().len() == 3);
    assert_eq!(
        *statuses.lock().unwrap(),
        vec![TaskStatus::Pending, TaskStatus::Running, TaskStatus::Done,]
    );
    assert_eq!(
        runtime.latest_snapshot("session-1"),
        Some(Snapshot { value: 7 })
    );
}

#[test]
fn missing_credentials_leave_runtime_unchanged() {
    let app = mock_app();
    let statuses = statuses(app.handle());
    let runtime = SamplingTaskRuntime::<Snapshot>::new(spec());
    let mut invalid_host = host();
    invalid_host.auth_type = AuthType::Password;
    invalid_host.password_ref = None;
    let worker_called = Arc::new(AtomicBool::new(false));
    let called = worker_called.clone();

    let result = runtime.start(
        app.handle().clone(),
        "session-1".to_string(),
        invalid_host,
        move |_, _| {
            called.store(true, Ordering::Release);
        },
    );

    assert!(matches!(result, Err(AppError::InvalidHostConfig(_))));
    assert!(!worker_called.load(Ordering::Acquire));
    assert!(statuses.lock().unwrap().is_empty());
    assert!(runtime.latest_snapshot("session-1").is_none());
}

#[test]
fn stop_session_rejects_late_start() {
    let app = mock_app();
    let statuses = statuses(app.handle());
    let runtime = SamplingTaskRuntime::<Snapshot>::new(spec());
    runtime.stop_session(app.handle(), "session-1");

    let result = runtime.start(
        app.handle().clone(),
        "session-1".to_string(),
        host(),
        |_, _| panic!("迟到 start 不应执行 worker"),
    );

    assert!(matches!(result, Err(AppError::SessionNotFound(_))));
    assert!(statuses.lock().unwrap().is_empty());
}

#[test]
fn stop_session_cleans_only_target_session_and_emits_done_once() {
    let app = mock_app();
    let statuses = statuses(app.handle());
    let runtime = SamplingTaskRuntime::<Snapshot>::new(spec());
    let (target_tx, target_rx) = std::sync::mpsc::channel();
    let (other_tx, other_rx) = std::sync::mpsc::channel();

    runtime
        .start(
            app.handle().clone(),
            "target".to_string(),
            host(),
            move |_, _| {
                target_rx.recv().unwrap();
            },
        )
        .unwrap();
    runtime
        .start(
            app.handle().clone(),
            "other".to_string(),
            host(),
            move |_, _| {
                other_rx.recv().unwrap();
            },
        )
        .unwrap();
    wait_until(|| statuses.lock().unwrap().len() == 4);
    runtime.insert_snapshot_for_test("target", Snapshot { value: 1 });
    runtime.insert_snapshot_for_test("other", Snapshot { value: 2 });

    runtime.stop_session(app.handle(), "target");
    wait_until(|| statuses.lock().unwrap().len() == 5);
    assert!(runtime.latest_snapshot("target").is_none());
    assert_eq!(
        runtime.latest_snapshot("other"),
        Some(Snapshot { value: 2 })
    );

    target_tx.send(()).unwrap();
    other_tx.send(()).unwrap();
    wait_until(|| statuses.lock().unwrap().len() == 6);
    let events = statuses.lock().unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|status| **status == TaskStatus::Pending)
            .count(),
        2
    );
    assert_eq!(
        events
            .iter()
            .filter(|status| **status == TaskStatus::Running)
            .count(),
        2
    );
    assert_eq!(
        events
            .iter()
            .filter(|status| **status == TaskStatus::Done)
            .count(),
        2
    );
}

#[test]
fn snapshot_emit_failure_stops_worker_and_fails_task_once() {
    let app = mock_app();
    let statuses = statuses(app.handle());
    let runtime = SamplingTaskRuntime::<Snapshot>::new(SamplingTaskSpec {
        snapshot_event: "invalid event",
        ..spec()
    });
    runtime
        .start(
            app.handle().clone(),
            "session-1".to_string(),
            host(),
            move |_, sink| {
                sink.publish(Snapshot { value: 3 });
            },
        )
        .unwrap();
    wait_until(|| statuses.lock().unwrap().len() == 3);

    assert_eq!(
        *statuses.lock().unwrap(),
        vec![TaskStatus::Pending, TaskStatus::Running, TaskStatus::Failed]
    );
    assert!(runtime.latest_snapshot("session-1").is_some());
}

#[test]
fn stop_suppresses_late_snapshot_failure_and_done() {
    let app = mock_app();
    let statuses = statuses(app.handle());
    let runtime = SamplingTaskRuntime::<Snapshot>::new(spec());
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let task = runtime
        .start(
            app.handle().clone(),
            "session-1".to_string(),
            host(),
            move |_, sink| {
                release_rx.recv().unwrap();
                sink.publish(Snapshot { value: 1 });
                sink.fail("测试采集失败: {0}", "late".to_string());
            },
        )
        .unwrap();

    wait_until(|| statuses.lock().unwrap().len() == 2);
    assert!(runtime.stop(app.handle(), &task.task_id));
    release_tx.send(()).unwrap();
    thread::sleep(Duration::from_millis(50));

    assert_eq!(
        *statuses.lock().unwrap(),
        vec![TaskStatus::Pending, TaskStatus::Running, TaskStatus::Done]
    );
    assert!(runtime.latest_snapshot("session-1").is_none());
}

#[test]
fn failed_task_rejects_late_done_and_panic_is_structured() {
    let app = mock_app();
    let statuses = statuses(app.handle());
    let runtime = SamplingTaskRuntime::<Snapshot>::new(spec());
    runtime
        .start(
            app.handle().clone(),
            "session-1".to_string(),
            host(),
            |_, sink| sink.fail("测试采集失败: {0}", "boom".to_string()),
        )
        .unwrap();
    wait_until(|| statuses.lock().unwrap().len() == 3);
    assert_eq!(
        *statuses.lock().unwrap(),
        vec![TaskStatus::Pending, TaskStatus::Running, TaskStatus::Failed]
    );

    let panic_runtime = SamplingTaskRuntime::<Snapshot>::new(spec());
    panic_runtime
        .start(
            app.handle().clone(),
            "session-2".to_string(),
            host(),
            |_, _| panic!("boom"),
        )
        .unwrap();
    wait_until(|| statuses.lock().unwrap().len() == 6);
    let events = statuses.lock().unwrap();
    assert_eq!(events[5], TaskStatus::Failed);
}

#[test]
fn poisoned_state_remains_usable() {
    let runtime = SamplingTaskRuntime::<Snapshot>::new(spec());
    let state = runtime.state.clone();
    let _ = thread::spawn(move || {
        let _guard = state.lock().unwrap();
        panic!("poison state");
    })
    .join();

    let app = mock_app();
    runtime
        .start(
            app.handle().clone(),
            "session-1".to_string(),
            host(),
            |_, _| {},
        )
        .unwrap();
}
