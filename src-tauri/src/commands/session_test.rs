#[cfg(test)]
mod tests {
    use crate::commands::session::run_host_lookup;
    use crate::errors::app_error::AppError;
    use std::sync::mpsc;
    use std::time::Duration;

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
