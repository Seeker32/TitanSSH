use crate::errors::app_error::AppError;
use ssh2::Channel;
use std::io::Write;

/// 向 SSH Channel 写入数据并立即刷新缓冲区
///
/// 将用户输入的字节序列写入 SSH 通道的 stdin，
/// 写入后立即 flush 确保数据及时发送到远端，避免缓冲延迟。
///
/// # 参数
/// - `channel`: 已建立的 SSH 通道可变引用
/// - `data`: 要写入的字符串数据（UTF-8 编码）
pub fn write_channel(channel: &mut Channel, data: &str) -> Result<(), AppError> {
    channel.write_all(data.as_bytes())?;
    channel.flush()?;
    Ok(())
}

/// 调整 SSH Channel 关联的 PTY 终端大小
///
/// 通知远端 SSH 服务器更新伪终端的列数和行数，
/// 使远端程序（如 vim、less）能正确感知终端尺寸变化。
///
/// # 参数
/// - `channel`: 已建立的 SSH 通道可变引用
/// - `cols`: 新的终端列数（字符宽度）
/// - `rows`: 新的终端行数（字符高度）
pub fn resize_channel(channel: &mut Channel, cols: u32, rows: u32) -> Result<(), AppError> {
    channel
        .request_pty_size(cols, rows, None, None)
        .map_err(AppError::from)
}
