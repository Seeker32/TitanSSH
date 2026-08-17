use crate::core::host_identity::{
    HostKeyVerifier, PresentedHostKey, algorithm_name, fingerprint_sha256,
};
use crate::errors::app_error::{AppError, ErrorDetail};
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
    /// 首次未知主机身份确认：阻塞等待用户决定，不进入认证
    VerifyingHostKey,
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

/// SFTP capability 的 module 内部 adapter seam；仅 crate 内可见。
pub(crate) trait SftpOps: Send {
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
    /// 将远端 src 重命名为 dst，供上传临时文件安全发布。
    ///
    /// overwrite = false：dst 已存在时返回 SftpTargetExists，绝不改动 dst；
    /// overwrite = true：原子替换 dst；远端无法保证安全替换时返回
    /// SftpPublishError 且不得先删除或改动 dst（旧目标保留）。
    fn rename(&mut self, src: &str, dst: &str, overwrite: bool) -> Result<(), AppError>;
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

    /// 将远端 src 重命名为 dst，供上传临时文件安全发布。
    pub fn rename(&mut self, src: &str, dst: &str, overwrite: bool) -> Result<(), AppError> {
        self.inner.rename(src, dst, overwrite)
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
            .map_err(|error| AppError::SftpTransferError(error.to_string().into()))
    }

    /// 删除 ssh2 远端文件。
    fn unlink(&mut self, path: &str) -> Result<(), AppError> {
        self.sftp
            .unlink(Path::new(path))
            .map_err(|error| AppError::SftpTransferError(error.to_string().into()))
    }

    /// 重命名 ssh2 远端文件：no-clobber（overwrite=false）或覆盖替换（overwrite=true）。
    ///
    /// SFTP v5+ 携带覆盖标志实现原子替换；v3 重命名无标志语义（OpenSSH 以
    /// link+unlink 实现，目标已存在即失败），libssh2 不会先删旧文件。
    fn rename(&mut self, src: &str, dst: &str, overwrite: bool) -> Result<(), AppError> {
        let flags = if overwrite {
            ssh2::RenameFlags::OVERWRITE
        } else {
            ssh2::RenameFlags::empty()
        };
        self.sftp
            .rename(Path::new(src), Path::new(dst), Some(flags))
            .map_err(|error| map_sftp_rename_error(dst, overwrite, error))
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
    verifier: &HostKeyVerifier,
    mut on_phase: F,
) -> Result<TerminalTransport, AppError>
where
    F: FnMut(ConnectPhase),
{
    let session = connect_session(host, password, passphrase, verifier, &mut on_phase)?;
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
    verifier: &HostKeyVerifier,
) -> Result<SftpTransport, AppError> {
    let session = connect_session(host, password, passphrase, verifier, &mut |_| {})?;
    let sftp = session
        .sftp()
        .map_err(|error| AppError::SftpChannelError(error.to_string().into()))?;
    Ok(SftpTransport::from_backend(Ssh2Sftp { sftp }))
}

/// 建立 Monitoring exec 专用 SSH 连接。
pub fn connect_exec(
    host: &HostConfig,
    password: Option<&str>,
    passphrase: Option<&str>,
    verifier: &HostKeyVerifier,
) -> Result<ExecTransport, AppError> {
    connect_session(host, password, passphrase, verifier, &mut |_| {})
        .map(|session| ExecTransport::from_backend(Ssh2Exec { session }))
}

/// 建立 TCP、SSH 握手并完成认证；raw Session 不离开本 module。
/// 首次未知主机身份在握手后、任何认证前经 verifier 统一校验，
/// 拒绝或取消时不发送任何密码或私钥。
fn connect_session<F>(
    host: &HostConfig,
    password: Option<&str>,
    passphrase: Option<&str>,
    verifier: &HostKeyVerifier,
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

    // 主机身份统一校验：计算指纹并等待用户决定；任何拒绝在认证前终止连接。
    // 服务器未呈现主机密钥同样不得进入认证（无法验证身份即不发送凭据）。
    on_phase(ConnectPhase::VerifyingHostKey);
    let (blob, key_type) = session.host_key().ok_or_else(|| {
        AppError::SshProtocolError(ErrorDetail::msg(
            "服务器未提供主机密钥，无法验证主机身份，已阻止认证",
            Vec::new(),
        ))
    })?;
    let presented = PresentedHostKey {
        host: host.host.clone(),
        port: host.port,
        algorithm: algorithm_name(key_type).to_string(),
        fingerprint: fingerprint_sha256(blob),
        blob: blob.to_vec(),
    };
    verifier(&presented)?;

    on_phase(ConnectPhase::Authenticating);
    match host.auth_type {
        AuthType::Password => {
            let password = password.ok_or_else(|| {
                AppError::InvalidHostConfig(ErrorDetail::msg("密码为必填项", Vec::new()))
            })?;
            session
                .userauth_password(&host.username, password)
                .map_err(|error| AppError::AuthenticationError(error.to_string().into()))?;
        }
        AuthType::PrivateKey => {
            let private_key = host.private_key_path.as_deref().ok_or_else(|| {
                AppError::InvalidHostConfig(ErrorDetail::msg("私钥路径为必填项", Vec::new()))
            })?;
            session
                .userauth_pubkey_file(&host.username, None, Path::new(private_key), passphrase)
                .map_err(|error| AppError::AuthenticationError(error.to_string().into()))?;
        }
    }

    if !session.authenticated() {
        return Err(AppError::AuthenticationError(ErrorDetail::msg(
            "SSH 认证失败",
            Vec::new(),
        )));
    }
    Ok(session)
}

/// 将 ssh2 错误转换为稳定应用错误文本。
fn protocol_error(error: ssh2::Error) -> AppError {
    AppError::SshProtocolError(error.to_string().into())
}

/// 将 ssh2 rename 错误转换为稳定领域错误。
///
/// 目标已存在：no-clobber 发布竞态 → SftpTargetExists（前端据此逐文件确认覆盖）；
/// 覆盖发布 → 远端不支持覆盖语义（SFTP v3 重命名无覆盖标志），返回 SftpPublishError
/// 且旧目标保持不动，绝不先删旧文件。其余失败统一为 SftpPublishError 保留底层诊断。
fn map_sftp_rename_error(dst: &str, overwrite: bool, error: ssh2::Error) -> AppError {
    let message = error.to_string();
    // LIBSSH2_FX_FILE_ALREADY_EXISTS = 11（libssh2_sftp.h 稳定常量）；
    // 消息兜底兼容错误码未能传递的服务端实现差异。
    let already_exists = matches!(error.code(), ssh2::ErrorCode::SFTP(11))
        || message.contains("File already exists");
    if already_exists {
        if overwrite {
            AppError::SftpPublishError(ErrorDetail::msg(
                "远端服务器无法保证安全替换，旧目标保留: {0} ({1})",
                vec![dst.to_string(), message],
            ))
        } else {
            AppError::SftpTargetExists(dst.to_string().into())
        }
    } else {
        AppError::SftpPublishError(ErrorDetail::msg(
            "远端重命名失败: {0} ({1})",
            vec![dst.to_string(), message],
        ))
    }
}

/// 将 SFTP 路径错误转换为稳定领域错误。
fn map_sftp_path_error(path: &str, error: ssh2::Error) -> AppError {
    let message = error.to_string();
    if message.contains("No such file") || message.contains("does not exist") {
        AppError::SftpPathNotFound(path.to_string().into())
    } else if message.contains("Permission denied") {
        AppError::SftpPermissionDenied(path.to_string().into())
    } else {
        AppError::SftpChannelError(message.into())
    }
}

/// 解析目标主机的所有可连接地址。
fn resolve_socket_addrs(host: &HostConfig) -> Result<Vec<SocketAddr>, AppError> {
    let address = format!("{}:{}", host.host, host.port);
    let socket_addrs: Vec<SocketAddr> = address.to_socket_addrs()?.collect();
    if socket_addrs.is_empty() {
        return Err(AppError::SshConnectionError(ErrorDetail::msg(
            "连接失败: 未解析到可用地址 {0}",
            vec![address],
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
        AppError::SshConnectionError(
            format!("Connection timeout after {}s", timeout.as_secs()).into(),
        )
    } else {
        AppError::SshConnectionError(ErrorDetail::msg(
            "连接失败: {0}",
            vec![
                last_error
                    .unwrap_or_else(|| io::Error::other("unknown TCP connection error"))
                    .to_string(),
            ],
        ))
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::{
        ExecOps, ExecTransport, RemoteFile, SftpEntry, SftpOps, SftpTransport, TerminalOps,
        TerminalTransport,
    };
    use crate::errors::app_error::{AppError, ErrorDetail};
    use std::collections::{HashMap, VecDeque};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Barrier, Condvar, Mutex};

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
            Err(AppError::SftpTransferError("unused".to_string().into()))
        }

        /// 空 adapter 不提供远端写文件。
        fn create(&mut self, _path: &str) -> Result<RemoteFile, AppError> {
            Err(AppError::SftpTransferError("unused".to_string().into()))
        }

        /// 空 adapter 的删除操作直接成功。
        fn unlink(&mut self, _path: &str) -> Result<(), AppError> {
            Ok(())
        }

        /// 本 adapter 不提供远端重命名。
        fn rename(&mut self, _src: &str, _dst: &str, _overwrite: bool) -> Result<(), AppError> {
            Err(AppError::SftpTransferError("unused".to_string().into()))
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
            Err(AppError::SftpTransferError("unused".to_string().into()))
        }

        /// 本 adapter 不创建远端文件。
        fn create(&mut self, _path: &str) -> Result<RemoteFile, AppError> {
            Err(AppError::SftpTransferError("unused".to_string().into()))
        }

        /// 本 adapter 无需清理远端文件。
        fn unlink(&mut self, _path: &str) -> Result<(), AppError> {
            Ok(())
        }

        /// 本 adapter 不提供远端重命名。
        fn rename(&mut self, _src: &str, _dst: &str, _overwrite: bool) -> Result<(), AppError> {
            Err(AppError::SftpTransferError("unused".to_string().into()))
        }
    }

    /// 远端读取在两段 barrier 之间阻塞的传输 adapter；list_dir/file_size 保持
    /// 轻量成功实现，仅供“传输占用传输连接时控制连接仍响应” contract 测试。
    struct BlockingReadSftp {
        started: Arc<Barrier>,
        release: Arc<Barrier>,
        blocked: Arc<AtomicBool>,
    }

    /// 首读在 barrier 之间阻塞、其后立即 EOF 的远端读句柄；阻塞状态由所属
    /// adapter 共享，同一传输连接上的后续读取不重复阻塞。
    struct BlockingReadFile {
        started: Arc<Barrier>,
        release: Arc<Barrier>,
        blocked: Arc<AtomicBool>,
    }

    impl std::io::Read for BlockingReadFile {
        /// 第一次读取在 barrier 之间阻塞，释放后返回 EOF 完成传输。
        fn read(&mut self, _buffer: &mut [u8]) -> std::io::Result<usize> {
            if !self.blocked.swap(true, Ordering::SeqCst) {
                self.started.wait();
                self.release.wait();
            }
            Ok(0)
        }
    }

    impl std::io::Write for BlockingReadFile {
        /// 本句柄不写入。
        fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
            Ok(buffer.len())
        }

        /// 本句柄不刷新。
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl SftpOps for BlockingReadSftp {
        /// 返回空目录。
        fn list_dir(&mut self, _path: &str) -> Result<Vec<SftpEntry>, AppError> {
            Ok(Vec::new())
        }

        /// 本 adapter 不查询文件大小（元数据走控制连接）。
        fn file_size(&mut self, _path: &str) -> Result<u64, AppError> {
            Ok(0)
        }

        /// 返回在 barrier 之间阻塞的远端读句柄。
        fn open_read(&mut self, _path: &str) -> Result<RemoteFile, AppError> {
            Ok(RemoteFile {
                inner: Box::new(BlockingReadFile {
                    started: self.started.clone(),
                    release: self.release.clone(),
                    blocked: self.blocked.clone(),
                }),
            })
        }

        /// 本 adapter 不创建远端文件。
        fn create(&mut self, _path: &str) -> Result<RemoteFile, AppError> {
            Err(AppError::SftpTransferError("unused".to_string().into()))
        }

        /// 本 adapter 无需删除远端文件。
        fn unlink(&mut self, _path: &str) -> Result<(), AppError> {
            Ok(())
        }

        /// 本 adapter 不提供远端重命名。
        fn rename(&mut self, _src: &str, _dst: &str, _overwrite: bool) -> Result<(), AppError> {
            Err(AppError::SftpTransferError("unused".to_string().into()))
        }
    }

    /// 传输打开/创建持续返回通道错误的 SFTP adapter，模拟已失效的传输连接。
    struct ChannelFailingTransferSftp;

    impl SftpOps for ChannelFailingTransferSftp {
        /// 返回空目录。
        fn list_dir(&mut self, _path: &str) -> Result<Vec<SftpEntry>, AppError> {
            Ok(Vec::new())
        }

        /// 返回非零大小。
        fn file_size(&mut self, _path: &str) -> Result<u64, AppError> {
            Ok(1)
        }

        /// 打开远端文件时返回通道错误。
        fn open_read(&mut self, _path: &str) -> Result<RemoteFile, AppError> {
            Err(AppError::SftpChannelError(
                "transfer channel lost".to_string().into(),
            ))
        }

        /// 创建远端文件时返回通道错误。
        fn create(&mut self, _path: &str) -> Result<RemoteFile, AppError> {
            Err(AppError::SftpChannelError(
                "transfer channel lost".to_string().into(),
            ))
        }

        /// 本 adapter 无需删除远端文件。
        fn unlink(&mut self, _path: &str) -> Result<(), AppError> {
            Ok(())
        }

        /// 本 adapter 不提供远端重命名。
        fn rename(&mut self, _src: &str, _dst: &str, _overwrite: bool) -> Result<(), AppError> {
            Err(AppError::SftpTransferError("unused".to_string().into()))
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

        /// 发布直接成功：内存 adapter 丢弃写入内容，无需真实落位。
        fn rename(&mut self, _src: &str, _dst: &str, _overwrite: bool) -> Result<(), AppError> {
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
            Err(AppError::SftpTransferError("unused".to_string().into()))
        }

        /// 本 adapter 不创建文件。
        fn create(&mut self, _path: &str) -> Result<RemoteFile, AppError> {
            Err(AppError::SftpTransferError("unused".to_string().into()))
        }

        /// 本 adapter 无需删除文件。
        fn unlink(&mut self, _path: &str) -> Result<(), AppError> {
            Ok(())
        }

        /// 本 adapter 不提供远端重命名。
        fn rename(&mut self, _src: &str, _dst: &str, _overwrite: bool) -> Result<(), AppError> {
            Err(AppError::SftpTransferError("unused".to_string().into()))
        }
    }

    impl ExecOps for OneShotExec {
        /// 返回固定 stdout 并结束 Monitoring 循环。
        fn execute(&mut self, _command: &str) -> Result<String, AppError> {
            self.shutdown.store(true, Ordering::Relaxed);
            Ok(self.output.clone())
        }
    }

    /// 上传写入放行门：阻塞的测试传输在此等待，测试显式放行控制完成时序；
    /// 首个写入到达时置位到达信号，供测试确定传输已进入写入阶段。
    pub(crate) struct Gate {
        released: Mutex<bool>,
        cond: Condvar,
        arrived: Mutex<bool>,
        arrived_cond: Condvar,
    }

    impl Gate {
        /// 创建未放行的门。
        pub(crate) fn new() -> Arc<Self> {
            Arc::new(Self {
                released: Mutex::new(false),
                cond: Condvar::new(),
                arrived: Mutex::new(false),
                arrived_cond: Condvar::new(),
            })
        }

        /// 放行所有在该门后等待的传输；放行后新写入立即成功。
        pub(crate) fn open(&self) {
            *self
                .released
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
            self.cond.notify_all();
        }

        /// 首个写入到达时置位到达信号并唤醒等待者（测试阻塞在 wait_arrived 上）。
        fn arrive(&self) {
            *self
                .arrived
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
            self.arrived_cond.notify_all();
        }

        /// 阻塞直到首个写入到达门（已到达时立即返回）。
        pub(crate) fn wait_arrived(&self) {
            let mut arrived = self
                .arrived
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            while !*arrived {
                arrived = self
                    .arrived_cond
                    .wait(arrived)
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
            }
        }
    }

    /// 上传写入在放行门后阻塞的传输 adapter；每个 transport 持有独立门，
    /// 供“五路并发/第六个等待”contract 测试控制每个传输的完成时序。
    pub(crate) struct GatedCreateSftp {
        pub(crate) released: Arc<Gate>,
    }

    /// 首次写入等待放行、放行后写入立即成功的远端写句柄。
    struct GatedWriteFile {
        released: Arc<Gate>,
    }

    impl std::io::Read for GatedWriteFile {
        /// 本句柄不产生输入。
        fn read(&mut self, _buffer: &mut [u8]) -> std::io::Result<usize> {
            Ok(0)
        }
    }

    impl std::io::Write for GatedWriteFile {
        /// 首次写入等待放行；放行后直接成功。
        fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
            let mut released = self
                .released
                .released
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            while !*released {
                released = self
                    .released
                    .cond
                    .wait(released)
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
            }
            Ok(buffer.len())
        }

        /// 本句柄不刷新。
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl SftpOps for GatedCreateSftp {
        /// 返回空目录。
        fn list_dir(&mut self, _path: &str) -> Result<Vec<SftpEntry>, AppError> {
            Ok(Vec::new())
        }

        /// 本 adapter 不查询文件大小。
        fn file_size(&mut self, _path: &str) -> Result<u64, AppError> {
            Ok(0)
        }

        /// 本 adapter 不打开远端读文件。
        fn open_read(&mut self, _path: &str) -> Result<RemoteFile, AppError> {
            Err(AppError::SftpTransferError("unused".to_string().into()))
        }

        /// 返回在放行门后阻塞写入的远端写句柄。
        fn create(&mut self, _path: &str) -> Result<RemoteFile, AppError> {
            Ok(RemoteFile {
                inner: Box::new(GatedWriteFile {
                    released: self.released.clone(),
                }),
            })
        }

        /// 本 adapter 无需删除远端文件。
        fn unlink(&mut self, _path: &str) -> Result<(), AppError> {
            Ok(())
        }

        /// 发布直接成功：门控 adapter 只控制写入时序，不模拟远端文件系统。
        fn rename(&mut self, _src: &str, _dst: &str, _overwrite: bool) -> Result<(), AppError> {
            Ok(())
        }
    }

    /// 内存远端文件系统 adapter：按路径存储内容，完整实现 rename 两种发布
    /// 语义、unlink 失败注入与可选写入放行门，供上传安全发布 contract 测试
    /// 观察远端文件系统的真实状态。全部状态在 Arc 字段中，跨连接共享同一文件系统。
    #[derive(Clone)]
    pub(crate) struct InMemorySftp {
        files: Arc<Mutex<HashMap<String, Vec<u8>>>>,
        /// false 模拟不支持原子替换的远端：overwrite rename 返回 SftpPublishError 且不改动 dst
        overwrite_supported: bool,
        /// true 时全部 unlink 返回清理失败错误（detail 含被删路径）
        unlink_denied: Arc<std::sync::atomic::AtomicBool>,
        /// true 时远端写入首次即失败（供运行时写入失败 + 清理失败合并测试）
        write_denied: bool,
        /// 首次写入在此门后阻塞（测试控制传输时序）
        gate: Option<Arc<Gate>>,
        /// 记录全部 unlink 调用路径，供“只清理本任务临时文件”断言
        unlink_calls: Arc<Mutex<Vec<String>>>,
    }

    impl InMemorySftp {
        /// 拒绝后续全部 unlink；被删路径写入错误 detail。
        pub(crate) fn deny_unlink(&self) {
            self.unlink_denied.store(true, Ordering::Relaxed);
        }

        /// 读取指定路径的远端内容；不存在返回 None。
        pub(crate) fn content(&self, path: &str) -> Option<Vec<u8>> {
            self.files
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .get(path)
                .cloned()
        }

        /// 判断远端文件是否存在。
        pub(crate) fn has_file(&self, path: &str) -> bool {
            self.files
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .contains_key(path)
        }

        /// 返回全部 unlink 调用路径（按调用顺序）。
        pub(crate) fn unlink_calls(&self) -> Vec<String> {
            self.unlink_calls
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone()
        }
    }

    /// 内存远端写句柄：写入累积到 buffer，flush 或 drop 时提交到所属文件系统。
    struct InMemoryWriteFile {
        path: String,
        buffer: Vec<u8>,
        files: Arc<Mutex<HashMap<String, Vec<u8>>>>,
        write_denied: bool,
        gate: Option<Arc<Gate>>,
    }

    impl std::io::Read for InMemoryWriteFile {
        /// 写句柄不产生输入。
        fn read(&mut self, _buffer: &mut [u8]) -> std::io::Result<usize> {
            Ok(0)
        }
    }

    impl std::io::Write for InMemoryWriteFile {
        /// 首次写入在放行门后阻塞；deny 时写入失败，否则追加到 buffer。
        fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
            if let Some(gate) = &self.gate {
                gate.arrive();
                let mut released = gate
                    .released
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                while !*released {
                    released = gate
                        .cond
                        .wait(released)
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                }
            }
            if self.write_denied {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::ConnectionReset,
                    "remote write reset",
                ));
            }
            self.buffer.extend_from_slice(buffer);
            Ok(buffer.len())
        }

        /// flush 即提交：模拟远端 fsync，之后 rename 才能看到完整内容。
        fn flush(&mut self) -> std::io::Result<()> {
            self.files
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .insert(self.path.clone(), self.buffer.clone());
            Ok(())
        }
    }

    impl Drop for InMemoryWriteFile {
        /// 关闭即提交：未 flush 的写入在句柄关闭时落位（零字节文件同样创建空条目）。
        fn drop(&mut self) {
            self.files
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .insert(self.path.clone(), std::mem::take(&mut self.buffer));
        }
    }

    impl SftpOps for InMemorySftp {
        /// 从内存文件系统列举目录条目（仅文件）。
        fn list_dir(&mut self, path: &str) -> Result<Vec<SftpEntry>, AppError> {
            let prefix = if path.ends_with('/') {
                path.to_string()
            } else {
                format!("{}/", path)
            };
            let files = self
                .files
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            Ok(files
                .iter()
                .filter(|(full, _)| {
                    full.starts_with(&prefix) && !full[prefix.len()..].contains('/')
                })
                .map(|(full, content)| SftpEntry {
                    path: full.clone(),
                    is_dir: false,
                    size: content.len() as u64,
                    modified_at: 0,
                    permissions: None,
                })
                .collect())
        }

        /// 查询内存文件大小；不存在返回 SftpPathNotFound。
        fn file_size(&mut self, path: &str) -> Result<u64, AppError> {
            match self.content(path) {
                Some(content) => Ok(content.len() as u64),
                None => Err(AppError::SftpPathNotFound(path.to_string().into())),
            }
        }

        /// 返回已提交内容的只读游标；不存在返回 SftpPathNotFound。
        fn open_read(&mut self, path: &str) -> Result<RemoteFile, AppError> {
            let content = self
                .content(path)
                .ok_or_else(|| AppError::SftpPathNotFound(path.to_string().into()))?;
            Ok(RemoteFile {
                inner: Box::new(std::io::Cursor::new(content)),
            })
        }

        /// 在内存文件系统中创建写句柄。
        fn create(&mut self, path: &str) -> Result<RemoteFile, AppError> {
            Ok(RemoteFile {
                inner: Box::new(InMemoryWriteFile {
                    path: path.to_string(),
                    buffer: Vec::new(),
                    files: self.files.clone(),
                    write_denied: self.write_denied,
                    gate: self.gate.clone(),
                }),
            })
        }

        /// 删除内存文件：deny 时返回包含路径的清理失败错误；调用路径始终记录。
        fn unlink(&mut self, path: &str) -> Result<(), AppError> {
            self.unlink_calls
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(path.to_string());
            if self.unlink_denied.load(Ordering::Relaxed) {
                return Err(AppError::SftpTransferError(
                    "permission denied".to_string().into(),
                ));
            }
            self.files
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .remove(path);
            Ok(())
        }

        /// 完整 rename 语义：no-clobber（overwrite=false）撞目标返回 SftpTargetExists；
        /// 覆盖发布在远端不支持原子替换时返回 SftpPublishError 且不改动 dst。
        fn rename(&mut self, src: &str, dst: &str, overwrite: bool) -> Result<(), AppError> {
            let mut files = self
                .files
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let Some(content) = files.get(src).cloned() else {
                return Err(AppError::SftpPathNotFound(src.to_string().into()));
            };
            let dst_exists = files.contains_key(dst);
            if dst_exists && !overwrite {
                return Err(AppError::SftpTargetExists(dst.to_string().into()));
            }
            if dst_exists && !self.overwrite_supported {
                return Err(AppError::SftpPublishError(ErrorDetail::msg(
                    "远端服务器无法保证安全替换，旧目标保留: {0}",
                    vec![dst.to_string()],
                )));
            }
            files.insert(dst.to_string(), content);
            files.remove(src);
            Ok(())
        }
    }

    /// 包装 adapter 的存活计数守卫：构造时计数加一、释放时减一，
    /// 供连接池回收测试观察 capability 的真实释放时机。
    struct LiveCountSftp<T: SftpOps> {
        inner: T,
        live: Arc<AtomicUsize>,
    }

    impl<T: SftpOps> LiveCountSftp<T> {
        /// 构造守卫并登记存活计数。
        fn counted(inner: T, live: Arc<AtomicUsize>) -> Self {
            live.fetch_add(1, Ordering::SeqCst);
            Self { inner, live }
        }
    }

    impl<T: SftpOps> Drop for LiveCountSftp<T> {
        /// 通知测试该 transport 已被释放。
        fn drop(&mut self) {
            self.live.fetch_sub(1, Ordering::SeqCst);
        }
    }

    impl<T: SftpOps> SftpOps for LiveCountSftp<T> {
        /// 委托给内部 adapter。
        fn list_dir(&mut self, path: &str) -> Result<Vec<SftpEntry>, AppError> {
            self.inner.list_dir(path)
        }

        /// 委托给内部 adapter。
        fn file_size(&mut self, path: &str) -> Result<u64, AppError> {
            self.inner.file_size(path)
        }

        /// 委托给内部 adapter。
        fn open_read(&mut self, path: &str) -> Result<RemoteFile, AppError> {
            self.inner.open_read(path)
        }

        /// 委托给内部 adapter。
        fn create(&mut self, path: &str) -> Result<RemoteFile, AppError> {
            self.inner.create(path)
        }

        /// 委托给内部 adapter。
        fn unlink(&mut self, path: &str) -> Result<(), AppError> {
            self.inner.unlink(path)
        }

        /// 委托给内部 adapter。
        fn rename(&mut self, src: &str, dst: &str, overwrite: bool) -> Result<(), AppError> {
            self.inner.rename(src, dst, overwrite)
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

    /// 目录与元数据操作持续返回通道错误的 SFTP adapter，模拟已失效的控制连接。
    struct FailingChannelSftp;

    impl SftpOps for FailingChannelSftp {
        /// 模拟失效连接：目录列举失败。
        fn list_dir(&mut self, _path: &str) -> Result<Vec<SftpEntry>, AppError> {
            Err(AppError::SftpChannelError(
                "connection lost".to_string().into(),
            ))
        }

        /// 模拟失效连接：元数据查询失败。
        fn file_size(&mut self, _path: &str) -> Result<u64, AppError> {
            Err(AppError::SftpChannelError(
                "connection lost".to_string().into(),
            ))
        }

        /// 本 adapter 不打开远端读文件。
        fn open_read(&mut self, _path: &str) -> Result<RemoteFile, AppError> {
            Err(AppError::SftpTransferError("unused".to_string().into()))
        }

        /// 本 adapter 不创建远端文件。
        fn create(&mut self, _path: &str) -> Result<RemoteFile, AppError> {
            Err(AppError::SftpTransferError("unused".to_string().into()))
        }

        /// 本 adapter 无需删除远端文件。
        fn unlink(&mut self, _path: &str) -> Result<(), AppError> {
            Ok(())
        }

        /// 本 adapter 不提供远端重命名。
        fn rename(&mut self, _src: &str, _dst: &str, _overwrite: bool) -> Result<(), AppError> {
            Err(AppError::SftpTransferError("unused".to_string().into()))
        }
    }

    /// 目录列举返回域错误（路径不存在）的 SFTP adapter，连接本身健康。
    struct PathNotFoundSftp;

    impl SftpOps for PathNotFoundSftp {
        /// 返回稳定的路径不存在域错误，供“域错误不触发重连”测试。
        fn list_dir(&mut self, path: &str) -> Result<Vec<SftpEntry>, AppError> {
            Err(AppError::SftpPathNotFound(path.to_string().into()))
        }

        /// 元数据查询返回同样的域错误。
        fn file_size(&mut self, path: &str) -> Result<u64, AppError> {
            Err(AppError::SftpPathNotFound(path.to_string().into()))
        }

        /// 本 adapter 不打开远端读文件。
        fn open_read(&mut self, _path: &str) -> Result<RemoteFile, AppError> {
            Err(AppError::SftpTransferError("unused".to_string().into()))
        }

        /// 本 adapter 不创建远端文件。
        fn create(&mut self, _path: &str) -> Result<RemoteFile, AppError> {
            Err(AppError::SftpTransferError("unused".to_string().into()))
        }

        /// 本 adapter 无需删除远端文件。
        fn unlink(&mut self, _path: &str) -> Result<(), AppError> {
            Ok(())
        }

        /// 本 adapter 不提供远端重命名。
        fn rename(&mut self, _src: &str, _dst: &str, _overwrite: bool) -> Result<(), AppError> {
            Err(AppError::SftpTransferError("unused".to_string().into()))
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
            Err(AppError::SftpTransferError("unused".to_string().into()))
        }

        /// 删除操作直接成功。
        fn unlink(&mut self, _path: &str) -> Result<(), AppError> {
            Ok(())
        }

        /// 本 adapter 不提供远端重命名。
        fn rename(&mut self, _src: &str, _dst: &str, _overwrite: bool) -> Result<(), AppError> {
            Err(AppError::SftpTransferError("unused".to_string().into()))
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
            Err(AppError::SftpTransferError("unused".to_string().into()))
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

        /// 本 adapter 不提供远端重命名。
        fn rename(&mut self, _src: &str, _dst: &str, _overwrite: bool) -> Result<(), AppError> {
            Err(AppError::SftpTransferError("unused".to_string().into()))
        }
    }

    /// 构建总是放行的主机身份校验器（真实 SSH E2E 与 transport 测试使用）。
    pub(crate) fn allow_all_verifier() -> crate::core::host_identity::HostKeyVerifier {
        std::sync::Arc::new(|_presented: &crate::core::host_identity::PresentedHostKey| Ok(()))
    }

    /// 构建空闲终端 capability：无输出、不 EOF，供连接编排测试驱动主循环。
    pub(crate) fn idle_terminal() -> TerminalTransport {
        struct IdleTerminal;
        impl TerminalOps for IdleTerminal {
            /// 始终无数据可读。
            fn read(&mut self, _buffer: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::new(std::io::ErrorKind::WouldBlock, "idle"))
            }
            /// 丢弃写入。
            fn write(&mut self, _data: &str) -> Result<(), AppError> {
                Ok(())
            }
            /// 忽略尺寸调整。
            fn resize(&mut self, _cols: u32, _rows: u32) -> Result<(), AppError> {
                Ok(())
            }
            /// 永不 EOF。
            fn eof(&self) -> bool {
                false
            }
            /// 关闭为 no-op。
            fn close(&mut self) -> Result<(), AppError> {
                Ok(())
            }
        }
        TerminalTransport::from_backend(IdleTerminal)
    }

    /// 构建写入必定失败但读取保持空闲的终端 capability，供终端断开契约测试使用。
    pub(crate) fn write_failing_terminal() -> TerminalTransport {
        struct WriteFailingTerminal;

        impl TerminalOps for WriteFailingTerminal {
            /// 始终无数据可读，确保测试仅由写入失败驱动断开。
            fn read(&mut self, _buffer: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::new(std::io::ErrorKind::WouldBlock, "idle"))
            }

            /// 模拟失效 SSH channel 的写入错误。
            fn write(&mut self, _data: &str) -> Result<(), AppError> {
                Err(AppError::SshConnectionError("terminal write failed".to_string().into()))
            }

            /// 测试 capability 忽略窗口大小调整。
            fn resize(&mut self, _cols: u32, _rows: u32) -> Result<(), AppError> {
                Ok(())
            }

            /// 保持非 EOF，确保实现不能依赖读路径发现断开。
            fn eof(&self) -> bool {
                false
            }

            /// 测试 capability 关闭为 no-op。
            fn close(&mut self) -> Result<(), AppError> {
                Ok(())
            }
        }

        TerminalTransport::from_backend(WriteFailingTerminal)
    }

    /// 构建按预设字节块读取的终端 capability，供终端流事件契约测试模拟分块读取。
    pub(crate) fn chunked_terminal(chunks: Vec<Vec<u8>>) -> TerminalTransport {
        struct ChunkedTerminal {
            chunks: VecDeque<Vec<u8>>,
        }

        impl TerminalOps for ChunkedTerminal {
            /// 将下一段预设字节复制到读取缓冲区，所有分块读完后返回零字节。
            fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
                let Some(chunk) = self.chunks.pop_front() else {
                    return Ok(0);
                };
                buffer[..chunk.len()].copy_from_slice(&chunk);
                Ok(chunk.len())
            }

            /// 测试 capability 不保留终端输入。
            fn write(&mut self, _data: &str) -> Result<(), AppError> {
                Ok(())
            }

            /// 测试 capability 忽略窗口大小调整。
            fn resize(&mut self, _cols: u32, _rows: u32) -> Result<(), AppError> {
                Ok(())
            }

            /// 预设分块全部读取后报告 EOF。
            fn eof(&self) -> bool {
                self.chunks.is_empty()
            }

            /// 测试 capability 关闭为 no-op。
            fn close(&mut self) -> Result<(), AppError> {
                Ok(())
            }
        }

        TerminalTransport::from_backend(ChunkedTerminal {
            chunks: chunks.into(),
        })
    }

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

    /// 创建目录/元数据操作持续返回通道错误的 SFTP 测试 capability，模拟失效控制连接。
    pub(crate) fn failing_channel_sftp() -> SftpTransport {
        SftpTransport::from_backend(FailingChannelSftp)
    }

    /// 创建目录/元数据操作返回路径不存在域错误的 SFTP 测试 capability。
    pub(crate) fn path_not_found_sftp() -> SftpTransport {
        SftpTransport::from_backend(PathNotFoundSftp)
    }

    /// 创建可控制阻塞时序的 SFTP 测试 capability。
    pub(crate) fn blocking_sftp(started: Arc<Barrier>, release: Arc<Barrier>) -> SftpTransport {
        SftpTransport::from_backend(BlockingSftp { started, release })
    }

    /// 创建读取在 barrier 之间阻塞的 SFTP 传输测试 capability，供传输/控制连接分离测试。
    pub(crate) fn blocking_read_sftp(
        started: Arc<Barrier>,
        release: Arc<Barrier>,
    ) -> SftpTransport {
        SftpTransport::from_backend(BlockingReadSftp {
            started,
            release,
            blocked: Arc::new(AtomicBool::new(false)),
        })
    }

    /// 创建传输打开/创建持续返回通道错误的 SFTP 测试 capability，模拟失效传输连接。
    pub(crate) fn channel_failing_transfer_sftp() -> SftpTransport {
        SftpTransport::from_backend(ChannelFailingTransferSftp)
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

    /// 创建支持原子替换的内存远端文件系统（预置初始文件）。
    pub(crate) fn in_memory_sftp(files: &[(&str, Vec<u8>)]) -> InMemorySftp {
        in_memory_sftp_with(files, true, false, None)
    }

    /// 创建不支持原子替换的内存远端文件系统：目标已存在时覆盖发布必然失败。
    pub(crate) fn in_memory_sftp_no_atomic_replace(files: &[(&str, Vec<u8>)]) -> InMemorySftp {
        in_memory_sftp_with(files, false, false, None)
    }

    /// 创建写入被放行门阻塞、deny 时写入失败的内存远端文件系统。
    pub(crate) fn gated_in_memory_sftp(
        files: &[(&str, Vec<u8>)],
        gate: Arc<Gate>,
        write_denied: bool,
    ) -> InMemorySftp {
        in_memory_sftp_with(files, true, write_denied, Some(gate))
    }

    /// 按参数装配内存远端文件系统 adapter。
    fn in_memory_sftp_with(
        files: &[(&str, Vec<u8>)],
        overwrite_supported: bool,
        write_denied: bool,
        gate: Option<Arc<Gate>>,
    ) -> InMemorySftp {
        InMemorySftp {
            files: Arc::new(Mutex::new(
                files
                    .iter()
                    .map(|(path, content)| (path.to_string(), content.clone()))
                    .collect(),
            )),
            overwrite_supported,
            unlink_denied: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            write_denied,
            gate,
            unlink_calls: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// 把内存远端文件系统包装为 SftpTransport；跨连接共享同一文件系统状态。
    pub(crate) fn in_memory_sftp_transport(fs: &InMemorySftp) -> SftpTransport {
        SftpTransport::from_backend(fs.clone())
    }

    /// 包装内部 adapter 的存活计数测试 capability；构造时计数加一，释放时减一。
    pub(crate) fn counted_sftp(
        inner: impl SftpOps + 'static,
        live: Arc<AtomicUsize>,
    ) -> SftpTransport {
        SftpTransport::from_backend(LiveCountSftp::counted(inner, live))
    }
}

#[cfg(test)]
#[path = "ssh_transport_test.rs"]
mod tests;
