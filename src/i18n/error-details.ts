/**
 * 后端错误详情模板的中文→英文翻译表（gettext msgid 风格）。
 *
 * key 是后端 ErrorDetail::msg 的中文模板（{0}/{1} 占位，与 zh-CN 展示一致），
 * value 是英文模板；查不到时前端回退展示中文模板（不静默丢失诊断）。
 * 新增后端中文模板后必须在此补充英文翻译。
 */
export const errorDetailPhrases: Record<string, string> = {
  // ── 主机配置 / 存储 ─────────────────────────────
  '主机名称为必填项': 'Host name is required',
  '主机地址为必填项': 'Host address is required',
  '用户名为必填项': 'Username is required',
  '密码为必填项': 'Password is required',
  '私钥路径为必填项': 'Private key path is required',
  '密码引用为空': 'Password reference is empty',
  '迁移旧主机配置失败: {0}': 'Could not migrate legacy host config: {0}',
  '无法获取应用数据目录: {0}': 'Could not access app data directory: {0}',
  '无法创建应用数据目录: {0}': 'Could not create app data directory: {0}',
  '读取主机配置文件失败: {0}': 'Could not read host config file: {0}',
  '解析主机配置文件失败: {0}': 'Could not parse host config file: {0}',
  '序列化主机配置失败: {0}': 'Could not serialize host config: {0}',
  '写入主机配置文件失败: {0}': 'Could not write host config file: {0}',

  // ── 信任存储 ───────────────────────────────────
  '信任存储未初始化，无法持久化信任记录': 'Trust store not initialized; cannot persist trust record',
  '信任存储未初始化，无法清理信任记录': 'Trust store not initialized; cannot clean up trust records',
  '信任存储路径无父目录: {0}': 'Trust store path has no parent directory: {0}',
  '读取信任存储失败: {0} ({1})': 'Could not read trust store: {0} ({1})',
  '解析信任存储失败: {0} 第 {1} 行 ({2})': 'Could not parse trust store: {0} at line {1} ({2})',
  '创建信任存储临时文件失败: {0}': 'Could not create trust store temp file: {0}',
  '写入信任存储临时文件失败: {0}': 'Could not write trust store temp file: {0}',
  '同步信任存储临时文件失败: {0}': 'Could not sync trust store temp file: {0}',
  '发布信任存储失败: {0} ({1})，原文件未受影响':
    'Could not publish trust store: {0} ({1}); original file left untouched',
  'endpoint {0}:{1} 的信任记录清理失败: {2}':
    'Failed to clean up trust record for endpoint {0}:{1}: {2}',

  // ── SSH 连接 / 传输 ─────────────────────────────
  'SSH 认证失败': 'SSH authentication failed',
  '服务器未提供主机密钥，无法验证主机身份，已阻止认证':
    'Server did not provide a host key; identity cannot be verified, authentication blocked',
  '连接失败: {0}': 'Connection failed: {0}',
  '连接失败: 未解析到可用地址 {0}': 'Connection failed: could not resolve any usable address {0}',
  '连接线程异常退出': 'Connection thread exited unexpectedly',
  'session 已关闭': 'Session closed',
  'session {0} 不存在': 'Session {0} does not exist',
  '全局传输信号量已关闭': 'Global transfer semaphore is closed',
  '传输连接槽丢失': 'Transfer connection slot lost',
  '传输连接槽为空': 'Transfer connection slot is empty',

  // ── SFTP ───────────────────────────────────────
  '本地路径无效': 'Invalid local path',
  '本地目录不存在: {0}': 'Local directory does not exist: {0}',
  '本地文件不存在: {0}': 'Local file does not exist: {0}',
  '清理临时文件失败: {0} ({1})': 'Could not clean up temp file: {0} ({1})',
  '打开临时文件失败: {0} ({1})': 'Could not open temp file: {0} ({1})',
  '登记临时文件失败: {0} ({1})': 'Could not register temp file: {0} ({1})',
  '远端重命名失败: {0} ({1})': 'Remote rename failed: {0} ({1})',
  '远端服务器无法保证安全替换，旧目标保留: {0}':
    'Server cannot guarantee atomic replacement; the old target was left untouched: {0}',
  '远端服务器无法保证安全替换，旧目标保留: {0} ({1})':
    'Server cannot guarantee atomic replacement; the old target was left untouched: {0} ({1})',
  '发布失败: {0} -> {1} ({2})，目标原文件未受影响':
    'Publish failed: {0} -> {1} ({2}); the original target file was left untouched',

  // ── 日志 ───────────────────────────────────────
  '无法获取应用日志目录: {0}': 'Could not access app log directory: {0}',

  // ── 监控 ───────────────────────────────────────
  '监控快照推送失败: {0}': 'Could not push monitor snapshot: {0}',
  '监控采集失败: {0}': 'Monitor collection failed: {0}',

  // ── 连接阶段超时 ───────────────────────────────
  '读取系统凭据超时': 'Loading credentials timed out',
  '建立 TCP 连接超时': 'Establishing TCP connection timed out',
  'SSH 握手超时': 'SSH handshake timed out',
  'SSH 认证超时': 'SSH authentication timed out',
  '打开终端通道超时': 'Opening terminal channel timed out',
  '请求终端 PTY 超时': 'Requesting terminal PTY timed out',
  '启动 Shell 超时': 'Starting shell timed out',
  '连接超时': 'Connection timed out',
};
