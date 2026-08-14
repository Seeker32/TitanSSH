pub mod host;
pub mod host_identity;
pub mod logging;
pub mod monitor;
pub mod session;
pub mod sftp;

use crate::errors::app_error::{AppError, AppErrorInfo};

/// 在阻塞线程池执行会等待远端 IO / 主机身份决定的命令操作。
///
/// Tauri 同步 command 运行在主线程：控制连接未就绪（TCP/SSH 握手、host-identity
/// challenge 未决）或 OS 安全存储等待授权时，主线程被占用会阻塞全部后续 invoke，
/// 前端表现为整体卡死。阻塞线程池线程数不受 async worker 数限制，未决等待不会
/// 饿死 async 运行时。
pub(crate) async fn run_blocking_op<T, F>(func: F) -> Result<T, AppErrorInfo>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, AppError> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(func)
        .await
        .map_err(|error| {
            AppErrorInfo::from(AppError::IoError(std::io::Error::other(format!(
                "阻塞操作线程异常退出: {error}"
            ))))
        })?
        .map_err(AppErrorInfo::from)
}

#[cfg(test)]
mod tests {
    use super::run_blocking_op;
    use crate::errors::app_error::AppError;
    use std::sync::mpsc;
    use std::time::Duration;

    /// 回归：会等待远端/挑战的操作必须在阻塞线程池执行，不得占用调用线程
    /// （真实应用中的 Tauri 主线程）。等待期间调用线程保持可用，可继续发送解除信号。
    #[test]
    fn run_blocking_op_executes_off_caller_thread() {
        let (started_tx, started_rx) = mpsc::channel::<std::thread::ThreadId>();
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let caller_id = std::thread::current().id();

        let task = tauri::async_runtime::spawn(run_blocking_op(move || {
            started_tx
                .send(std::thread::current().id())
                .expect("线程 ID 应送达");
            release_rx.recv().expect("release 信号应送达");
            Ok::<i32, AppError>(7)
        }));

        let worker_id = started_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("任务应已开始");
        assert_ne!(worker_id, caller_id, "阻塞操作不得占用调用线程");

        release_tx
            .send(())
            .expect("调用线程必须不被阻塞，可立即解除等待");
        let result = tauri::async_runtime::block_on(task)
            .expect("任务应正常完成")
            .expect("操作应成功");
        assert_eq!(result, 7);
    }

    /// 阻塞操作失败时结构化错误原样上抛，错误代码稳定。
    #[test]
    fn run_blocking_op_propagates_structured_error() {
        let task = tauri::async_runtime::spawn(run_blocking_op(move || {
            Err::<i32, AppError>(AppError::SftpChannelError("remote gone".to_string()))
        }));
        let error = tauri::async_runtime::block_on(task)
            .expect("任务应正常完成")
            .expect_err("操作错误应上抛");
        assert_eq!(error.code, "SftpChannelError");
    }
}
