use crate::errors::app_error::AppError;
use crate::models::host::{AuthType, HostConfig};
use serde::Serialize;
use ssh2::{Channel, Session, Sftp};
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::path::Path;
use std::time::Duration;

/// SSH TCP 建连固定超时时间，避免调用方无限等待。
const SSH_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// SSH 握手与认证阶段超时时间，单位毫秒。
const SSH_PROTOCOL_TIMEOUT_MS: u32 = 10_000;
/// Terminal channel 初始化阶段超时时间，单位毫秒。
const TERMINAL_SETUP_TIMEOUT_MS: u32 = 5_000;

/// SSH transport 建连阶段，供 Terminal 保持既有诊断事件。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum ConnectPhase {
    ConnectingTcp,
    SshHandshake,
    Authenticating,
    OpeningChannel,
    RequestingPty,
    StartingShell,
}

/// Terminal capability 的 module 内部 adapter seam。
trait TerminalOps: Send {
    /// 读取一批终端输出字节。
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize>;
    /// 写入终端输入并刷新。
    fn write(&mut self, data: &str) -> Result<(), AppError>;
    /// 调整远端 PTY 尺寸。
    fn resize(&mut self, cols: u32, rows: u32) -> Result<(), AppError>;
    /// 判断远端是否已结束输出。
    fn eof(&self) -> bool;
    /// 关闭远端终端 channel。
    fn close(&mut self) -> Result<(), AppError>;
}

/// Terminal 专用 opaque capability；调用方无法访问 ssh2 Session 或 Channel。
pub struct TerminalTransport {
    inner: Box<dyn TerminalOps>,
}

impl TerminalTransport {
    /// 用 module 内部 adapter 构造 capability，生产与测试共用同一 interface。
    fn from_backend(inner: impl TerminalOps + 'static) -> Self {
        Self {
            inner: Box::new(inner),
        }
    }

    /// 读取一批终端输出字节。
    pub fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.inner.read(buffer)
    }

    /// 写入终端输入并刷新。
    pub fn write(&mut self, data: &str) -> Result<(), AppError> {
        self.inner.write(data)
    }

    /// 调整远端 PTY 尺寸。
    pub fn resize(&mut self, cols: u32, rows: u32) -> Result<(), AppError> {
        self.inner.resize(cols, rows)
    }

    /// 判断远端是否已结束输出。
    pub fn eof(&self) -> bool {
        self.inner.eof()
    }

    /// 关闭远端终端 channel。
    pub fn close(&mut self) -> Result<(), AppError> {
        self.inner.close()
    }
}

/// SFTP 目录项的 transport-neutral 数据。
pub struct SftpEntry {
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
    pub modified_at: i64,
    pub permissions: Option<u32>,
}

/// 隐藏 ssh2::File 的远端文件句柄。
pub struct RemoteFile {
    inner: Box<dyn RemoteIo>,
}

trait RemoteIo: Read + Write + Send {}
impl<T: Read + Write + Send> RemoteIo for T {}

impl Read for RemoteFile {
    /// 从远端文件读取字节。
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.inner.read(buffer)
    }
}

impl Write for RemoteFile {
    /// 向远端文件写入字节。
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.inner.write(buffer)
    }

    /// 刷新远端文件写入。
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// SFTP capability 的 module 内部 adapter seam。
trait SftpOps: Send {
    /// 读取远端目录。
    fn list_dir(&mut self, path: &str) -> Result<Vec<SftpEntry>, AppError>;
    /// 查询远端文件大小。
    fn file_size(&mut self, path: &str) -> Result<u64, AppError>;
    /// 打开远端文件用于读取。
    fn open_read(&mut self, path: &str) -> Result<RemoteFile, AppError>;
    /// 创建远端文件用于写入。
    fn create(&mut self, path: &str) -> Result<RemoteFile, AppError>;
    /// 删除远端文件，供失败上传清理残留。
    fn unlink(&mut self, path: &str) -> Result<(), AppError>;
}

/// SFTP 专用 opaque capability；连接始终保持 blocking，不影响 Terminal。
pub struct SftpTransport {
    inner: Box<dyn SftpOps>,
}

impl SftpTransport {
    /// 用 module 内部 adapter 构造 capability，生产与测试共用同一 interface。
    fn from_backend(inner: impl SftpOps + 'static) -> Self {
        Self {
            inner: Box::new(inner),
        }
    }

    /// 读取远端目录。
    pub fn list_dir(&mut self, path: &str) -> Result<Vec<SftpEntry>, AppError> {
        self.inner.list_dir(path)
    }

    /// 查询远端文件大小。
    pub fn file_size(&mut self, path: &str) -> Result<u64, AppError> {
        self.inner.file_size(path)
    }

    /// 打开远端文件用于读取。
    pub fn open_read(&mut self, path: &str) -> Result<RemoteFile, AppError> {
        self.inner.open_read(path)
    }

    /// 创建远端文件用于写入。
    pub fn create(&mut self, path: &str) -> Result<RemoteFile, AppError> {
        self.inner.create(path)
    }

    /// 删除远端文件，供失败上传清理残留。
    pub fn unlink(&mut self, path: &str) -> Result<(), AppError> {
        self.inner.unlink(path)
    }
}

/// Monitoring exec capability 的 module 内部 adapter seam。
trait ExecOps: Send {
    /// 执行一个远端命令并读取完整 stdout。
    fn execute(&mut self, command: &str) -> Result<String, AppError>;
}

/// Monitoring 专用 opaque capability；调用方只看到命令执行行为。
pub struct ExecTransport {
    inner: Box<dyn ExecOps>,
}

impl ExecTransport {
    /// 用 module 内部 adapter 构造 capability，生产与测试共用同一 interface。
    fn from_backend(inner: impl ExecOps + 'static) -> Self {
        Self {
            inner: Box::new(inner),
        }
    }

    /// 执行一个远端命令并读取完整 stdout。
    pub fn execute(&mut self, command: &str) -> Result<String, AppError> {
        self.inner.execute(command)
    }
}

struct Ssh2Terminal {
    channel: Channel,
}

impl TerminalOps for Ssh2Terminal {
    /// 从 ssh2 Channel 读取输出。
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.channel.read(buffer)
    }

    /// 写入 ssh2 Channel 并刷新。
    fn write(&mut self, data: &str) -> Result<(), AppError> {
        self.channel.write_all(data.as_bytes())?;
        self.channel.flush()?;
        Ok(())
    }

    /// 调整 ssh2 PTY 尺寸。
    fn resize(&mut self, cols: u32, rows: u32) -> Result<(), AppError> {
        self.channel
            .request_pty_size(cols, rows, None, None)
            .map_err(protocol_error)
    }

    /// 判断 ssh2 Channel 是否 EOF。
    fn eof(&self) -> bool {
        self.channel.eof()
    }

    /// 关闭 ssh2 Channel。
    fn close(&mut self) -> Result<(), AppError> {
        self.channel.close().map_err(protocol_error)
    }
}

struct Ssh2Sftp {
    sftp: Sftp,
}

impl SftpOps for Ssh2Sftp {
    /// 读取并转换 ssh2 目录项。
    fn list_dir(&mut self, path: &str) -> Result<Vec<SftpEntry>, AppError> {
        self.sftp
            .readdir(Path::new(path))
            .map_err(|error| map_sftp_path_error(path, error))?
            .into_iter()
            .map(|(path, stat)| {
                Ok(SftpEntry {
                    path: path.to_string_lossy().to_string(),
                    is_dir: stat.is_dir(),
                    size: if stat.is_dir() {
                        0
                    } else {
                        stat.size.unwrap_or(0)
                    },
                    modified_at: stat.mtime.map(|time| time as i64 * 1000).unwrap_or(0),
                    permissions: stat.perm,
                })
            })
            .collect()
    }

    /// 查询 ssh2 远端文件大小。
    fn file_size(&mut self, path: &str) -> Result<u64, AppError> {
        self.sftp
            .stat(Path::new(path))
            .map(|stat| stat.size.unwrap_or(0))
            .map_err(|error| map_sftp_path_error(path, error))
    }

    /// 打开 ssh2 远端文件用于读取。
    fn open_read(&mut self, path: &str) -> Result<RemoteFile, AppError> {
        self.sftp
            .open(Path::new(path))
            .map(|file| RemoteFile {
                inner: Box::new(file),
            })
            .map_err(|error| map_sftp_path_error(path, error))
    }

    /// 创建 ssh2 远端文件用于写入。
    fn create(&mut self, path: &str) -> Result<RemoteFile, AppError> {
        self.sftp
            .create(Path::new(path))
            .map(|file| RemoteFile {
                inner: Box::new(file),
            })
            .map_err(|error| AppError::SftpTransferError(error.to_string()))
    }

    /// 删除 ssh2 远端文件。
    fn unlink(&mut self, path: &str) -> Result<(), AppError> {
        self.sftp
            .unlink(Path::new(path))
            .map_err(|error| AppError::SftpTransferError(error.to_string()))
    }
}

struct Ssh2Exec {
    session: Session,
}

impl ExecOps for Ssh2Exec {
    /// 通过独立 ssh2 Session 执行命令并读取 stdout。
    fn execute(&mut self, command: &str) -> Result<String, AppError> {
        let mut channel = self.session.channel_session().map_err(protocol_error)?;
        channel.exec(command).map_err(protocol_error)?;
        let mut output = String::new();
        channel.read_to_string(&mut output)?;
        channel.wait_close().map_err(protocol_error)?;
        Ok(output)
    }
}

/// 建立并初始化 Terminal 专用 SSH 连接。
pub fn connect_terminal<F>(
    host: &HostConfig,
    password: Option<&str>,
    passphrase: Option<&str>,
    mut on_phase: F,
) -> Result<TerminalTransport, AppError>
where
    F: FnMut(ConnectPhase),
{
    let session = connect_session(host, password, passphrase, &mut on_phase)?;
    session.set_timeout(TERMINAL_SETUP_TIMEOUT_MS);

    on_phase(ConnectPhase::OpeningChannel);
    let mut channel = session.channel_session().map_err(protocol_error)?;
    on_phase(ConnectPhase::RequestingPty);
    channel
        .request_pty("xterm", None, Some((120, 32, 0, 0)))
        .map_err(protocol_error)?;
    on_phase(ConnectPhase::StartingShell);
    channel.shell().map_err(protocol_error)?;
    session.set_blocking(false);

    Ok(TerminalTransport::from_backend(Ssh2Terminal { channel }))
}

/// 建立并初始化 SFTP 专用 SSH 连接。
pub fn connect_sftp(
    host: &HostConfig,
    password: Option<&str>,
    passphrase: Option<&str>,
) -> Result<SftpTransport, AppError> {
    let session = connect_session(host, password, passphrase, &mut |_| {})?;
    let sftp = session
        .sftp()
        .map_err(|error| AppError::SftpChannelError(error.to_string()))?;
    Ok(SftpTransport::from_backend(Ssh2Sftp { sftp }))
}

/// 建立 Monitoring exec 专用 SSH 连接。
pub fn connect_exec(
    host: &HostConfig,
    password: Option<&str>,
    passphrase: Option<&str>,
) -> Result<ExecTransport, AppError> {
    connect_session(host, password, passphrase, &mut |_| {})
        .map(|session| ExecTransport::from_backend(Ssh2Exec { session }))
}

/// 建立 TCP、SSH 握手并完成认证；raw Session 不离开本 module。
fn connect_session<F>(
    host: &HostConfig,
    password: Option<&str>,
    passphrase: Option<&str>,
    on_phase: &mut F,
) -> Result<Session, AppError>
where
    F: FnMut(ConnectPhase),
{
    on_phase(ConnectPhase::ConnectingTcp);
    let socket_addrs = resolve_socket_addrs(host)?;
    let tcp = connect_tcp_stream(&socket_addrs, SSH_CONNECT_TIMEOUT)?;
    tcp.set_read_timeout(Some(Duration::from_millis(SSH_PROTOCOL_TIMEOUT_MS.into())))?;
    tcp.set_write_timeout(Some(Duration::from_millis(SSH_PROTOCOL_TIMEOUT_MS.into())))?;

    on_phase(ConnectPhase::SshHandshake);
    let mut session = Session::new().map_err(protocol_error)?;
    session.set_timeout(SSH_PROTOCOL_TIMEOUT_MS);
    session.set_tcp_stream(tcp);
    session.handshake().map_err(protocol_error)?;

    on_phase(ConnectPhase::Authenticating);
    match host.auth_type {
        AuthType::Password => {
            let password =
                password.ok_or_else(|| AppError::InvalidHostConfig("密码为必填项".to_string()))?;
            session
                .userauth_password(&host.username, password)
                .map_err(|error| AppError::AuthenticationError(error.to_string()))?;
        }
        AuthType::PrivateKey => {
            let private_key = host
                .private_key_path
                .as_deref()
                .ok_or_else(|| AppError::InvalidHostConfig("私钥路径为必填项".to_string()))?;
            session
                .userauth_pubkey_file(&host.username, None, Path::new(private_key), passphrase)
                .map_err(|error| AppError::AuthenticationError(error.to_string()))?;
        }
    }

    if !session.authenticated() {
        return Err(AppError::AuthenticationError("SSH 认证失败".to_string()));
    }
    Ok(session)
}

/// 将 ssh2 错误转换为稳定应用错误文本。
fn protocol_error(error: ssh2::Error) -> AppError {
    AppError::SshProtocolError(error.to_string())
}

/// 将 SFTP 路径错误转换为稳定领域错误。
fn map_sftp_path_error(path: &str, error: ssh2::Error) -> AppError {
    let message = error.to_string();
    if message.contains("No such file") || message.contains("does not exist") {
        AppError::SftpPathNotFound(path.to_string())
    } else if message.contains("Permission denied") {
        AppError::SftpPermissionDenied(path.to_string())
    } else {
        AppError::SftpChannelError(message)
    }
}

/// 解析目标主机的所有可连接地址。
fn resolve_socket_addrs(host: &HostConfig) -> Result<Vec<SocketAddr>, AppError> {
    let address = format!("{}:{}", host.host, host.port);
    let socket_addrs: Vec<SocketAddr> = address.to_socket_addrs()?.collect();
    if socket_addrs.is_empty() {
        return Err(AppError::SshConnectionError(format!(
            "连接失败: 未解析到可用地址 {address}"
        )));
    }
    Ok(socket_addrs)
}

/// 使用固定超时逐个尝试 TCP 建连。
fn connect_tcp_stream(
    socket_addrs: &[SocketAddr],
    timeout: Duration,
) -> Result<TcpStream, AppError> {
    let mut last_error = None;
    let mut saw_timeout = false;
    for socket_addr in socket_addrs {
        match TcpStream::connect_timeout(socket_addr, timeout) {
            Ok(stream) => return Ok(stream),
            Err(error) => {
                saw_timeout |= is_timeout_error(&error);
                last_error = Some(error);
            }
        }
    }
    Err(build_connect_error(saw_timeout, last_error, timeout))
}

/// 判断底层 IO 错误是否属于连接超时。
fn is_timeout_error(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
    )
}

/// 将多地址 TCP 尝试结果归一为稳定连接错误。
fn build_connect_error(
    saw_timeout: bool,
    last_error: Option<io::Error>,
    timeout: Duration,
) -> AppError {
    if saw_timeout {
        AppError::SshConnectionError(format!("Connection timeout after {}s", timeout.as_secs()))
    } else {
        AppError::SshConnectionError(format!(
            "连接失败: {}",
            last_error.unwrap_or_else(|| io::Error::other("unknown TCP connection error"))
        ))
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::{ExecOps, ExecTransport, RemoteFile, SftpEntry, SftpOps, SftpTransport};
    use crate::errors::app_error::AppError;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Barrier};

    struct EmptySftp;

    impl SftpOps for EmptySftp {
        /// 返回空目录用于所属 module 的行为测试。
        fn list_dir(&mut self, _path: &str) -> Result<Vec<SftpEntry>, AppError> {
            Ok(Vec::new())
        }

        /// 返回零长度用于所属 module 的行为测试。
        fn file_size(&mut self, _path: &str) -> Result<u64, AppError> {
            Ok(0)
        }

        /// 空 adapter 不提供远端读文件。
        fn open_read(&mut self, _path: &str) -> Result<RemoteFile, AppError> {
            Err(AppError::SftpTransferError("unused".to_string()))
        }

        /// 空 adapter 不提供远端写文件。
        fn create(&mut self, _path: &str) -> Result<RemoteFile, AppError> {
            Err(AppError::SftpTransferError("unused".to_string()))
        }

        /// 空 adapter 的删除操作直接成功。
        fn unlink(&mut self, _path: &str) -> Result<(), AppError> {
            Ok(())
        }
    }

    struct BlockingSftp {
        started: Arc<Barrier>,
        release: Arc<Barrier>,
    }

    impl SftpOps for BlockingSftp {
        /// 在两个 barrier 之间阻塞目录读取，供并发 contract 测试控制时序。
        fn list_dir(&mut self, _path: &str) -> Result<Vec<SftpEntry>, AppError> {
            self.started.wait();
            self.release.wait();
            Ok(Vec::new())
        }

        /// 本 adapter 不查询文件大小。
        fn file_size(&mut self, _path: &str) -> Result<u64, AppError> {
            Ok(0)
        }

        /// 本 adapter 不打开远端读文件。
        fn open_read(&mut self, _path: &str) -> Result<RemoteFile, AppError> {
            Err(AppError::SftpTransferError("unused".to_string()))
        }

        /// 本 adapter 不创建远端文件。
        fn create(&mut self, _path: &str) -> Result<RemoteFile, AppError> {
            Err(AppError::SftpTransferError("unused".to_string()))
        }

        /// 本 adapter 无需清理远端文件。
        fn unlink(&mut self, _path: &str) -> Result<(), AppError> {
            Ok(())
        }
    }

    struct OneShotExec {
        output: String,
        shutdown: Arc<AtomicBool>,
    }

    struct DropSignalSftp {
        dropped: std::sync::mpsc::Sender<()>,
    }

    /// 内存 SFTP adapter：open_read 返回固定内容，create 接受写入后丢弃。
    struct MemorySftp {
        content: Vec<u8>,
    }

    impl SftpOps for MemorySftp {
        /// 返回空目录。
        fn list_dir(&mut self, _path: &str) -> Result<Vec<SftpEntry>, AppError> {
            Ok(Vec::new())
        }

        /// 返回固定内容的字节数。
        fn file_size(&mut self, _path: &str) -> Result<u64, AppError> {
            Ok(self.content.len() as u64)
        }

        /// 返回固定内容的只读游标。
        fn open_read(&mut self, _path: &str) -> Result<RemoteFile, AppError> {
            Ok(RemoteFile {
                inner: Box::new(std::io::Cursor::new(self.content.clone())),
            })
        }

        /// 返回丢弃写入的空游标，使传输成功完成。
        fn create(&mut self, _path: &str) -> Result<RemoteFile, AppError> {
            Ok(RemoteFile {
                inner: Box::new(std::io::Cursor::new(Vec::new())),
            })
        }

        /// 删除操作直接成功。
        fn unlink(&mut self, _path: &str) -> Result<(), AppError> {
            Ok(())
        }
    }

    impl Drop for DropSignalSftp {
        /// 通知测试 capability 已被释放。
        fn drop(&mut self) {
            let _ = self.dropped.send(());
        }
    }

    impl SftpOps for DropSignalSftp {
        /// 返回空目录。
        fn list_dir(&mut self, _path: &str) -> Result<Vec<SftpEntry>, AppError> {
            Ok(Vec::new())
        }

        /// 返回零长度。
        fn file_size(&mut self, _path: &str) -> Result<u64, AppError> {
            Ok(0)
        }

        /// 本 adapter 不打开文件。
        fn open_read(&mut self, _path: &str) -> Result<RemoteFile, AppError> {
            Err(AppError::SftpTransferError("unused".to_string()))
        }

        /// 本 adapter 不创建文件。
        fn create(&mut self, _path: &str) -> Result<RemoteFile, AppError> {
            Err(AppError::SftpTransferError("unused".to_string()))
        }

        /// 本 adapter 无需删除文件。
        fn unlink(&mut self, _path: &str) -> Result<(), AppError> {
            Ok(())
        }
    }

    impl ExecOps for OneShotExec {
        /// 返回固定 stdout 并结束 Monitoring 循环。
        fn execute(&mut self, _command: &str) -> Result<String, AppError> {
            self.shutdown.store(true, Ordering::Relaxed);
            Ok(self.output.clone())
        }
    }

    /// 首读即失败的远端读句柄，供运行时读取失败测试。
    struct FailingReadFile;

    impl std::io::Read for FailingReadFile {
        /// 每次读取都返回连接重置错误。
        fn read(&mut self, _buffer: &mut [u8]) -> std::io::Result<usize> {
            Err(std::io::Error::new(
                std::io::ErrorKind::ConnectionReset,
                "remote read reset",
            ))
        }
    }

    impl std::io::Write for FailingReadFile {
        /// 本句柄不写入。
        fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
            Ok(buffer.len())
        }

        /// 本句柄不刷新。
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// 首次写入即失败的远端写句柄，供运行时写入失败测试。
    struct FailingWriteFile;

    impl std::io::Read for FailingWriteFile {
        /// 本句柄不产生输入。
        fn read(&mut self, _buffer: &mut [u8]) -> std::io::Result<usize> {
            Ok(0)
        }
    }

    impl std::io::Write for FailingWriteFile {
        /// 每次写入都返回连接重置错误。
        fn write(&mut self, _buffer: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::new(
                std::io::ErrorKind::ConnectionReset,
                "remote write reset",
            ))
        }

        /// 本句柄不刷新。
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// 远端读取在打开后失败的 SFTP adapter。
    struct FailingReadSftp;

    impl SftpOps for FailingReadSftp {
        /// 返回空目录。
        fn list_dir(&mut self, _path: &str) -> Result<Vec<SftpEntry>, AppError> {
            Ok(Vec::new())
        }

        /// 返回非零大小使下载 worker 进入读取循环。
        fn file_size(&mut self, _path: &str) -> Result<u64, AppError> {
            Ok(64)
        }

        /// 打开成功，但读取立即失败。
        fn open_read(&mut self, _path: &str) -> Result<RemoteFile, AppError> {
            Ok(RemoteFile {
                inner: Box::new(FailingReadFile),
            })
        }

        /// 本 adapter 不创建远端文件。
        fn create(&mut self, _path: &str) -> Result<RemoteFile, AppError> {
            Err(AppError::SftpTransferError("unused".to_string()))
        }

        /// 删除操作直接成功。
        fn unlink(&mut self, _path: &str) -> Result<(), AppError> {
            Ok(())
        }
    }

    /// 远端写入在创建后失败的 SFTP adapter。
    struct FailingWriteSftp;

    impl SftpOps for FailingWriteSftp {
        /// 返回空目录。
        fn list_dir(&mut self, _path: &str) -> Result<Vec<SftpEntry>, AppError> {
            Ok(Vec::new())
        }

        /// 返回零长度。
        fn file_size(&mut self, _path: &str) -> Result<u64, AppError> {
            Ok(0)
        }

        /// 本 adapter 不打开远端读文件。
        fn open_read(&mut self, _path: &str) -> Result<RemoteFile, AppError> {
            Err(AppError::SftpTransferError("unused".to_string()))
        }

        /// 创建成功，但写入立即失败。
        fn create(&mut self, _path: &str) -> Result<RemoteFile, AppError> {
            Ok(RemoteFile {
                inner: Box::new(FailingWriteFile),
            })
        }

        /// 删除操作直接成功。
        fn unlink(&mut self, _path: &str) -> Result<(), AppError> {
            Ok(())
        }
    }

    /// 创建返回空目录的 SFTP 测试 capability。
    pub(crate) fn empty_sftp() -> SftpTransport {
        SftpTransport::from_backend(EmptySftp)
    }

    /// 创建打开成功但读取失败的 SFTP 测试 capability，供运行时读取失败测试。
    pub(crate) fn failing_read_sftp() -> SftpTransport {
        SftpTransport::from_backend(FailingReadSftp)
    }

    /// 创建创建成功但写入失败的 SFTP 测试 capability，供运行时写入失败测试。
    pub(crate) fn failing_write_sftp() -> SftpTransport {
        SftpTransport::from_backend(FailingWriteSftp)
    }

    /// 创建可控制阻塞时序的 SFTP 测试 capability。
    pub(crate) fn blocking_sftp(started: Arc<Barrier>, release: Arc<Barrier>) -> SftpTransport {
        SftpTransport::from_backend(BlockingSftp { started, release })
    }

    /// 创建只返回一轮 stdout 的 Exec 测试 capability。
    pub(crate) fn one_shot_exec(output: String, shutdown: Arc<AtomicBool>) -> ExecTransport {
        ExecTransport::from_backend(OneShotExec { output, shutdown })
    }

    /// 创建在释放时发送信号的 SFTP 测试 capability。
    pub(crate) fn drop_signal_sftp(dropped: std::sync::mpsc::Sender<()>) -> SftpTransport {
        SftpTransport::from_backend(DropSignalSftp { dropped })
    }

    /// 创建支持内存读写（丢弃写入内容）的 SFTP 测试 capability，供 worker 全链路测试。
    pub(crate) fn memory_sftp(content: Vec<u8>) -> SftpTransport {
        SftpTransport::from_backend(MemorySftp { content })
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ConnectPhase, RemoteFile, SSH_PROTOCOL_TIMEOUT_MS, SftpEntry, SftpOps, SftpTransport,
        TerminalOps, TerminalTransport, build_connect_error, connect_tcp_stream, is_timeout_error,
        resolve_socket_addrs,
    };
    use crate::errors::app_error::AppError;
    use crate::models::host::{AuthType, HostConfig};
    use std::io;
    use std::net::SocketAddr;
    use std::sync::{Arc, Barrier, Mutex};
    use std::time::{Duration, Instant};

    struct RecordingTerminal {
        writes: Arc<Mutex<Vec<String>>>,
    }

    struct BlockingSftp {
        started: Arc<Barrier>,
        release: Arc<Barrier>,
    }

    impl SftpOps for BlockingSftp {
        /// 阻塞 SFTP 操作，验证它不会占用 Terminal capability。
        fn list_dir(&mut self, _path: &str) -> Result<Vec<SftpEntry>, AppError> {
            self.started.wait();
            self.release.wait();
            Ok(Vec::new())
        }

        /// 本测试不查询文件大小。
        fn file_size(&mut self, _path: &str) -> Result<u64, AppError> {
            Ok(0)
        }

        /// 本测试不打开文件。
        fn open_read(&mut self, _path: &str) -> Result<RemoteFile, AppError> {
            Err(AppError::SftpTransferError("unused".to_string()))
        }

        /// 本测试不创建文件。
        fn create(&mut self, _path: &str) -> Result<RemoteFile, AppError> {
            Err(AppError::SftpTransferError("unused".to_string()))
        }

        /// 本测试无需删除文件。
        fn unlink(&mut self, _path: &str) -> Result<(), AppError> {
            Ok(())
        }
    }

    impl TerminalOps for RecordingTerminal {
        /// 测试 adapter 不产生远端输出。
        fn read(&mut self, _buffer: &mut [u8]) -> io::Result<usize> {
            Ok(0)
        }

        /// 测试 adapter 记录写入内容。
        fn write(&mut self, data: &str) -> Result<(), AppError> {
            self.writes.lock().unwrap().push(data.to_string());
            Ok(())
        }

        /// 测试 adapter 接受任意终端尺寸。
        fn resize(&mut self, _cols: u32, _rows: u32) -> Result<(), AppError> {
            Ok(())
        }

        /// 测试 adapter 始终保持打开状态。
        fn eof(&self) -> bool {
            false
        }

        /// 测试 adapter 可无错误关闭。
        fn close(&mut self) -> Result<(), AppError> {
            Ok(())
        }
    }

    /// Terminal capability 只暴露行为，并将实现细节留在 transport module 内。
    #[test]
    fn terminal_capability_delegates_without_exposing_ssh2() {
        let writes = Arc::new(Mutex::new(Vec::new()));
        let mut terminal = TerminalTransport::from_backend(RecordingTerminal {
            writes: writes.clone(),
        });

        terminal.write("uptime\n").unwrap();

        assert_eq!(*writes.lock().unwrap(), vec!["uptime\n"]);
    }

    /// 阻塞的 SFTP adapter 不得阻塞独立 Terminal capability。
    #[test]
    fn blocking_sftp_does_not_block_terminal_capability() {
        let started = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let mut sftp = SftpTransport::from_backend(BlockingSftp {
            started: started.clone(),
            release: release.clone(),
        });
        let writes = Arc::new(Mutex::new(Vec::new()));
        let mut terminal = TerminalTransport::from_backend(RecordingTerminal {
            writes: writes.clone(),
        });

        let transfer = std::thread::spawn(move || sftp.list_dir("/"));
        started.wait();
        let before = Instant::now();
        terminal.write("echo responsive\n").unwrap();
        let elapsed = before.elapsed();
        release.wait();

        assert!(elapsed < Duration::from_millis(100));
        assert_eq!(*writes.lock().unwrap(), vec!["echo responsive\n"]);
        assert!(transfer.join().unwrap().is_ok());
    }

    /// 构造密码认证测试主机。
    fn make_host(host: &str, port: u16) -> HostConfig {
        HostConfig {
            id: "host-1".to_string(),
            name: "test".to_string(),
            host: host.to_string(),
            port,
            username: "root".to_string(),
            auth_type: AuthType::Password,
            password_ref: Some("ref".to_string()),
            private_key_path: None,
            passphrase_ref: None,
            remark: None,
            group: String::new(),
        }
    }

    /// 非法主机名必须返回解析错误。
    #[test]
    fn resolve_socket_addrs_returns_error_for_invalid_host() {
        assert!(resolve_socket_addrs(&make_host("invalid host name with spaces", 22)).is_err());
    }

    /// 非超时网络错误保持连接失败语义。
    #[test]
    fn connect_tcp_stream_returns_connection_error_without_timeout() {
        let address: SocketAddr = "127.0.0.1:1".parse().unwrap();
        let result = connect_tcp_stream(&[address], Duration::from_millis(50));

        assert!(matches!(
            result,
            Err(AppError::SshConnectionError(message)) if message.contains("连接失败")
        ));
    }

    /// 任一地址超时时优先保留 timeout 语义。
    #[test]
    fn build_connect_error_prefers_timeout_error() {
        let error = build_connect_error(
            true,
            Some(io::Error::new(io::ErrorKind::ConnectionRefused, "refused")),
            Duration::from_secs(10),
        );

        assert!(matches!(
            error,
            AppError::SshConnectionError(message) if message.contains("Connection timeout")
        ));
    }

    /// timeout 分类覆盖 TimedOut 与 WouldBlock，但不误判拒绝连接。
    #[test]
    fn is_timeout_error_recognizes_timeout_kinds() {
        assert!(is_timeout_error(&io::Error::new(
            io::ErrorKind::TimedOut,
            "timed out"
        )));
        assert!(is_timeout_error(&io::Error::new(
            io::ErrorKind::WouldBlock,
            "would block"
        )));
        assert!(!is_timeout_error(&io::Error::new(
            io::ErrorKind::ConnectionRefused,
            "refused"
        )));
    }

    /// ConnectPhase 序列化名称保持现有事件契约。
    #[test]
    fn connect_phase_serializes_to_stable_variant_name() {
        assert_eq!(
            serde_json::to_string(&ConnectPhase::Authenticating).unwrap(),
            "\"Authenticating\""
        );
    }

    /// SSH 协议超时保持十秒。
    #[test]
    fn ssh_protocol_timeout_is_ten_seconds() {
        assert_eq!(SSH_PROTOCOL_TIMEOUT_MS, 10_000);
    }

    /// 从环境变量构造真实 SSH E2E 主机与运行时凭据。
    fn e2e_host() -> (HostConfig, Option<String>, Option<String>) {
        let host = std::env::var("TITAN_SSH_E2E_HOST").expect("缺少 TITAN_SSH_E2E_HOST");
        let username =
            std::env::var("TITAN_SSH_E2E_USERNAME").expect("缺少 TITAN_SSH_E2E_USERNAME");
        let port = std::env::var("TITAN_SSH_E2E_PORT")
            .ok()
            .map(|value| value.parse().expect("TITAN_SSH_E2E_PORT 必须是 u16"))
            .unwrap_or(22);
        let private_key_path = std::env::var("TITAN_SSH_E2E_PRIVATE_KEY_PATH").ok();
        let password = std::env::var("TITAN_SSH_E2E_PASSWORD").ok();
        let passphrase = std::env::var("TITAN_SSH_E2E_PASSPHRASE").ok();
        let auth_type = if private_key_path.is_some() {
            AuthType::PrivateKey
        } else {
            assert!(password.is_some(), "密码认证缺少 TITAN_SSH_E2E_PASSWORD");
            AuthType::Password
        };

        (
            HostConfig {
                id: "ssh-e2e".to_string(),
                name: "ssh-e2e".to_string(),
                host,
                port,
                username,
                auth_type,
                password_ref: password.as_ref().map(|_| "env-password".to_string()),
                private_key_path,
                passphrase_ref: passphrase.as_ref().map(|_| "env-passphrase".to_string()),
                remark: None,
                group: String::new(),
            },
            password,
            passphrase,
        )
    }

    /// 真实 SSH E2E：慢速 SFTP 读取期间 Terminal marker 必须持续到达。
    #[test]
    #[ignore = "需要配置 TITAN_SSH_E2E_* 并访问真实 SSH server"]
    fn real_terminal_stream_continues_during_sftp_transfer() {
        use std::io::Read;
        use std::sync::atomic::{AtomicBool, Ordering};

        let (host, password, passphrase) = e2e_host();
        let mut exec = super::connect_exec(&host, password.as_deref(), passphrase.as_deref())
            .expect("Exec transport 应连接成功");
        let remote_path = format!("/tmp/titan-transport-{}.bin", uuid::Uuid::new_v4());
        exec.execute(&format!(
            "dd if=/dev/zero of={} bs=1048576 count=8 2>/dev/null",
            remote_path
        ))
        .expect("应创建 E2E 远端文件");

        let mut terminal =
            super::connect_terminal(&host, password.as_deref(), passphrase.as_deref(), |_| {})
                .expect("Terminal transport 应连接成功");
        let mut sftp = super::connect_sftp(&host, password.as_deref(), passphrase.as_deref())
            .expect("SFTP transport 应连接成功");
        let transfer_started = Arc::new(Barrier::new(2));
        let transfer_done = Arc::new(AtomicBool::new(false));
        let started_for_transfer = transfer_started.clone();
        let done_for_transfer = transfer_done.clone();
        let remote_for_transfer = remote_path.clone();
        let transfer = std::thread::spawn(move || {
            let mut remote = sftp
                .open_read(&remote_for_transfer)
                .expect("应打开 E2E 远端文件");
            started_for_transfer.wait();
            let mut buffer = [0_u8; 32 * 1024];
            while remote.read(&mut buffer).expect("SFTP 读取应成功") > 0 {
                std::thread::sleep(Duration::from_millis(10));
            }
            done_for_transfer.store(true, Ordering::Release);
        });

        transfer_started.wait();
        terminal
            .write("sleep 1; echo TITAN_CONCURRENT_MARKER\n")
            .expect("Terminal 写入应成功");
        let deadline = Instant::now() + Duration::from_secs(6);
        let mut output = String::new();
        let mut marker_arrived_during_transfer = false;
        let mut buffer = [0_u8; 4096];
        while Instant::now() < deadline {
            match terminal.read(&mut buffer) {
                Ok(size) if size > 0 => {
                    output.push_str(&String::from_utf8_lossy(&buffer[..size]));
                    if output.contains("TITAN_CONCURRENT_MARKER")
                        && !transfer_done.load(Ordering::Acquire)
                    {
                        marker_arrived_during_transfer = true;
                        break;
                    }
                }
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                Err(error) => panic!("Terminal 读取失败: {error}"),
            }
            std::thread::sleep(Duration::from_millis(20));
        }

        transfer.join().expect("SFTP 线程应正常退出");
        let _ = terminal.close();
        exec.execute(&format!("rm -f {}", remote_path))
            .expect("应清理 E2E 远端文件");
        assert!(
            marker_arrived_during_transfer,
            "SFTP 完成前应收到 Terminal marker，实际输出: {output}"
        );
    }
}
