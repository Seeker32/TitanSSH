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
#[path = "mod_test.rs"]
mod tests;
