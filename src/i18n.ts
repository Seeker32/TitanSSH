/** 应用支持的界面语言。 */
export type Locale = 'zh-CN' | 'en-US';

/** 后端跨 Tauri 边界返回的稳定错误。 */
export interface AppErrorInfo {
  code: string;
  detail?: string | null;
}

const zhCN = {
  'settings.title': '设置', 'settings.language': '语言', 'settings.terminalTheme': 'SSH 终端主题', 'settings.selected': '已选择',
  'terminalTheme.light': '浅色', 'terminalTheme.dark': '深色', 'terminalTheme.oneDark': 'One Dark', 'terminalTheme.dracula': 'Dracula', 'terminalTheme.solarizedLight': 'Solarized Light', 'terminalTheme.solarizedDark': 'Solarized Dark',
  'locale.zh-CN': '简体中文', 'locale.en-US': 'English',
  'theme.toggle': '切换主题', 'host.search': '搜索主机…', 'host.create': '新建主机', 'host.createFirst': '新建第一个主机',
  'host.empty': '暂无主机，点击下方按钮添加第一个 SSH 连接', 'host.noMatch': '未找到匹配的主机', 'host.ungrouped': '未分组',
  'host.renameGroup': '重命名分组', 'host.deleteGroup': '删除分组（主机归入未分组）', 'host.edit': '编辑主机', 'host.delete': '删除主机',
  'host.createConnection': '新建连接', 'host.editConnection': '编辑连接', 'host.save': '保存连接', 'common.cancel': '取消',
  'host.name': '名称', 'host.address': '地址', 'host.port': '端口', 'host.username': '用户名', 'host.auth': '认证方式', 'host.password': '密码',
  'host.privateKey': '私钥', 'host.privateKeyPath': '私钥路径', 'host.passphrase': '私钥口令', 'host.group': '分组', 'host.remark': '备注',
  'host.passwordAuth': '密码', 'host.privateKeyAuth': '私钥', 'host.namePlaceholder': '生产服务器', 'host.passwordPlaceholder': '留空则保持原密码不变',
  'host.keyRequired': '请先选择私钥文件', 'host.keyPlaceholder': '点击浏览选择私钥文件', 'host.browse': '浏览…', 'host.passphrasePlaceholder': '留空则保持原口令不变',
  'host.groupPlaceholder': '分组（可输入新组名）', 'host.remarkPlaceholder': '业务说明 / 环境标签', 'host.keyDialog': '选择私钥文件',
  'empty.title': '选择左侧主机开始连接', 'empty.hint': '双击主机卡片打开 SSH 会话，或先添加一台主机', 'tab.close': '关闭 {name}',
  'sftp.explorer': '文件浏览器', 'sftp.queue': '传输队列', 'sftp.selectSession': '请选择会话', 'sftp.upload': '上传', 'sftp.download': '下载',
  'sftp.loading': '加载中...', 'sftp.empty': '空目录', 'sftp.noTasks': '暂无传输任务', 'sftp.cancel': '取消', 'sftp.retry': '重新发起',
  'sftp.pending': '等待中', 'sftp.running': '传输中', 'sftp.done': '完成', 'sftp.failed': '失败', 'sftp.cancelled': '已取消', 'sftp.defaultDownload': '下载',
  'monitor.title': '服务器状态', 'monitor.connected': '已连接', 'monitor.disconnected': '未连接', 'monitor.name': '监控', 'monitor.collapse': '折叠监视条',
  'monitor.capacity': '剩余 {available} / 总量 {total}', 'monitor.unavailable': '网络数据不可用', 'monitor.noInterface': '无可用网卡', 'monitor.interface': '网卡接口',
  'monitor.down': '下行 · {name}', 'monitor.up': '上行 · {name}', 'monitor.legend': '趋势图例', 'monitor.downTrend': '下行趋势', 'monitor.upTrend': '上行趋势',
  'monitor.trend': '最近一分钟网卡速率趋势', 'monitor.ago': '60 秒前', 'monitor.now': '现在', 'monitor.empty': '连接建立后，这里会每 2 秒刷新一次服务器状态',
  'session.ready': '就绪', 'session.connecting': '正在连接 {name}', 'session.connectingGeneric': '正在连接...', 'session.authFailed': '认证失败，请检查用户名和密码',
  'session.timeout': '连接超时，请检查网络或主机地址', 'session.error': '连接错误', 'session.disconnected': '连接已断开', 'session.unknown': '连接异常',
  'phase.LoadingCredentials': '正在读取凭据...', 'phase.ConnectingTcp': '正在建立 TCP 连接...', 'phase.SshHandshake': '正在进行 SSH 握手...',
  'phase.Authenticating': '正在进行 SSH 认证...', 'phase.OpeningChannel': '正在打开终端通道...', 'phase.RequestingPty': '正在请求终端 PTY...', 'phase.StartingShell': '正在启动 Shell...',
  'error.AuthenticationError': '认证失败', 'error.SshConnectionError': 'SSH 连接失败', 'error.SessionNotFound': '会话不存在', 'error.InvalidHostConfig': '主机配置无效',
  'error.StorageError': '存储错误', 'error.IoError': 'IO 错误', 'error.SshProtocolError': 'SSH 协议错误', 'error.SecureStoreError': '安全存储错误',
  'error.CredentialNotFound': '凭据不存在', 'error.SftpChannelError': 'SFTP 通道错误', 'error.SftpPermissionDenied': 'SFTP 权限拒绝',
  'error.SftpPathNotFound': 'SFTP 路径不存在', 'error.SftpTransferError': 'SFTP 传输错误', 'error.MonitorError': '监控错误', 'error.Unknown': '未知错误',
} as const;

const enUS: Record<keyof typeof zhCN, string> = {
  'settings.title': 'Settings', 'settings.language': 'Language', 'settings.terminalTheme': 'SSH Terminal Theme', 'settings.selected': 'Selected',
  'terminalTheme.light': 'Light', 'terminalTheme.dark': 'Dark', 'terminalTheme.oneDark': 'One Dark', 'terminalTheme.dracula': 'Dracula', 'terminalTheme.solarizedLight': 'Solarized Light', 'terminalTheme.solarizedDark': 'Solarized Dark',
  'locale.zh-CN': 'Simplified Chinese', 'locale.en-US': 'English', 'theme.toggle': 'Toggle theme', 'host.search': 'Search hosts…', 'host.create': 'New host', 'host.createFirst': 'Create first host',
  'host.empty': 'No hosts yet. Add your first SSH connection below.', 'host.noMatch': 'No matching hosts', 'host.ungrouped': 'Ungrouped',
  'host.renameGroup': 'Rename group', 'host.deleteGroup': 'Delete group (hosts become ungrouped)', 'host.edit': 'Edit host', 'host.delete': 'Delete host',
  'host.createConnection': 'New connection', 'host.editConnection': 'Edit connection', 'host.save': 'Save connection', 'common.cancel': 'Cancel',
  'host.name': 'Name', 'host.address': 'Address', 'host.port': 'Port', 'host.username': 'Username', 'host.auth': 'Authentication', 'host.password': 'Password',
  'host.privateKey': 'Private key', 'host.privateKeyPath': 'Private key path', 'host.passphrase': 'Passphrase', 'host.group': 'Group', 'host.remark': 'Notes',
  'host.passwordAuth': 'Password', 'host.privateKeyAuth': 'Private key', 'host.namePlaceholder': 'Production server', 'host.passwordPlaceholder': 'Leave blank to keep the current password',
  'host.keyRequired': 'Select a private key first', 'host.keyPlaceholder': 'Browse to select a private key', 'host.browse': 'Browse…', 'host.passphrasePlaceholder': 'Leave blank to keep the current passphrase',
  'host.groupPlaceholder': 'Group (or enter a new name)', 'host.remarkPlaceholder': 'Service details / environment tag', 'host.keyDialog': 'Select private key',
  'empty.title': 'Select a host on the left to connect', 'empty.hint': 'Double-click a host to open an SSH session, or add a host first', 'tab.close': 'Close {name}',
  'sftp.explorer': 'File browser', 'sftp.queue': 'Transfer queue', 'sftp.selectSession': 'Select a session', 'sftp.upload': 'Upload', 'sftp.download': 'Download',
  'sftp.loading': 'Loading...', 'sftp.empty': 'Empty directory', 'sftp.noTasks': 'No transfer tasks', 'sftp.cancel': 'Cancel', 'sftp.retry': 'Retry',
  'sftp.pending': 'Pending', 'sftp.running': 'Transferring', 'sftp.done': 'Done', 'sftp.failed': 'Failed', 'sftp.cancelled': 'Cancelled', 'sftp.defaultDownload': 'download',
  'monitor.title': 'Server status', 'monitor.connected': 'Connected', 'monitor.disconnected': 'Disconnected', 'monitor.name': 'Monitor', 'monitor.collapse': 'Collapse monitor',
  'monitor.capacity': 'Available {available} / Total {total}', 'monitor.unavailable': 'Network data unavailable', 'monitor.noInterface': 'No network interface available', 'monitor.interface': 'Network interface',
  'monitor.down': 'Download · {name}', 'monitor.up': 'Upload · {name}', 'monitor.legend': 'Trend legend', 'monitor.downTrend': 'Download trend', 'monitor.upTrend': 'Upload trend',
  'monitor.trend': 'Network rate over the last minute', 'monitor.ago': '60 seconds ago', 'monitor.now': 'Now', 'monitor.empty': 'Server status refreshes here every 2 seconds after the connection is established',
  'session.ready': 'Ready', 'session.connecting': 'Connecting to {name}', 'session.connectingGeneric': 'Connecting...', 'session.authFailed': 'Authentication failed. Check the username and password.',
  'session.timeout': 'Connection timed out. Check the network or host address.', 'session.error': 'Connection error', 'session.disconnected': 'Connection disconnected', 'session.unknown': 'Connection failed',
  'phase.LoadingCredentials': 'Loading credentials...', 'phase.ConnectingTcp': 'Establishing TCP connection...', 'phase.SshHandshake': 'Performing SSH handshake...',
  'phase.Authenticating': 'Authenticating SSH...', 'phase.OpeningChannel': 'Opening terminal channel...', 'phase.RequestingPty': 'Requesting terminal PTY...', 'phase.StartingShell': 'Starting shell...',
  'error.AuthenticationError': 'Authentication failed', 'error.SshConnectionError': 'SSH connection failed', 'error.SessionNotFound': 'Session not found', 'error.InvalidHostConfig': 'Invalid host configuration',
  'error.StorageError': 'Storage error', 'error.IoError': 'I/O error', 'error.SshProtocolError': 'SSH protocol error', 'error.SecureStoreError': 'Secure storage error',
  'error.CredentialNotFound': 'Credential not found', 'error.SftpChannelError': 'SFTP channel error', 'error.SftpPermissionDenied': 'SFTP permission denied',
  'error.SftpPathNotFound': 'SFTP path not found', 'error.SftpTransferError': 'SFTP transfer error', 'error.MonitorError': 'Monitor error', 'error.Unknown': 'Unknown error',
};

export type TranslationKey = keyof typeof zhCN;
const dictionaries: Record<Locale, Record<TranslationKey, string>> = { 'zh-CN': zhCN, 'en-US': enUS };

/** 按语言读取文案并替换简单命名参数。 */
export function translate(locale: Locale, key: TranslationKey, params: Record<string, string | number> = {}): string {
  return dictionaries[locale][key].replace(/\{(\w+)\}/g, (_, name: string) => String(params[name] ?? `{${name}}`));
}

/** 格式化后端错误，保留诊断详情用于排障。 */
export function formatAppError(locale: Locale, error: AppErrorInfo | null | undefined): string {
  if (!error) return translate(locale, 'error.Unknown');
  const key = `error.${error.code}` as TranslationKey;
  const summary = key in dictionaries[locale] ? translate(locale, key) : translate(locale, 'error.Unknown');
  return error.detail?.trim() ? `${summary}: ${error.detail.trim()}` : summary;
}

/** 将 Tauri command rejection 规范为结构化错误。 */
export function toAppError(error: unknown): AppErrorInfo {
  if (error && typeof error === 'object' && 'code' in error) {
    const value = error as { code: unknown; detail?: unknown };
    return { code: typeof value.code === 'string' ? value.code : 'Unknown', detail: typeof value.detail === 'string' ? value.detail : null };
  }
  return { code: 'Unknown', detail: error instanceof Error ? error.message : String(error) };
}
