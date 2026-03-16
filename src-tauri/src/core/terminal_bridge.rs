use crate::errors::app_error::AppError;
use ssh2::Channel;
use std::io::Write;

pub fn write_channel(channel: &mut Channel, data: &str) -> Result<(), AppError> {
    channel.write_all(data.as_bytes())?;
    channel.flush()?;
    Ok(())
}

pub fn resize_channel(channel: &mut Channel, cols: u32, rows: u32) -> Result<(), AppError> {
    channel
        .request_pty_size(cols, rows, None, None)
        .map_err(AppError::from)
}
