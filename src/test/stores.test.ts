import { beforeEach, describe, expect, it, vi } from 'vitest';
import { invoke } from '@tauri-apps/api/core';
import { emitMockEvent, resetMockEvents } from '@tauri-apps/api/event';
import { filterHosts, groupHosts, useHostStore } from '@/stores/host';
import { DEFAULT_SIDEBAR_WIDTH, MIN_MAIN_PANEL_WIDTH, MIN_SIDEBAR_WIDTH, readCollapsedGroups, readMonitorCollapsed, useLayoutStore } from '@/stores/layout';
import { useMonitorStore } from '@/stores/monitor';
import { connectionLabel, useSessionStore } from '@/stores/session';
import { useSftpStore } from '@/stores/sftp';
import { ConnectionPhase, SessionStatus } from '@/types/session';
import { terminalTabId } from '@/types/tab';
import { TaskStatus } from '@/types/monitor';
import { uploadTargetDir, type RemoteEntry, type TransferTask } from '@/types/sftp';
import { makeHost, makeRemoteEntry, makeSession, makeSnapshot, makeTaskInfo, makeTerminalTab, makeTransferTask } from './fixtures';

const mockInvoke = vi.mocked(invoke);

/** 重置所有 Zustand store 和 Tauri 边界 mock。 */
function resetStores() {
  useHostStore.setState(useHostStore.getInitialState(), true);
  useLayoutStore.setState(useLayoutStore.getInitialState(), true);
  useMonitorStore.setState(useMonitorStore.getInitialState(), true);
  useSessionStore.setState(useSessionStore.getInitialState(), true);
  useSftpStore.setState(useSftpStore.getInitialState(), true);
  useSftpStore.getState().ensureState('session-1');
}

beforeEach(() => {
  resetStores();
  resetMockEvents();
  mockInvoke.mockReset();
  Object.defineProperty(window, 'innerWidth', { configurable: true, value: 1200 });
});

describe('Zustand stores', () => {
  it('加载、保存和删除主机均通过 Tauri 契约更新状态', async () => {
    const host = makeHost();
    mockInvoke.mockResolvedValueOnce([host]).mockResolvedValueOnce([host]).mockResolvedValueOnce([]);
    await useHostStore.getState().loadHosts();
    await useHostStore.getState().saveHost({ ...host, authType: host.authType, password: 'secret' });
    await useHostStore.getState().deleteHost(host.id);
    expect(mockInvoke).toHaveBeenNthCalledWith(1, 'list_hosts');
    expect(mockInvoke).toHaveBeenNthCalledWith(2, 'save_host', { request: expect.objectContaining({ group: 'production' }) });
    expect(mockInvoke).toHaveBeenNthCalledWith(3, 'delete_host', { hostId: host.id });
    expect(useHostStore.getState().hosts).toEqual([]);
  });

  it('主机加载失败保留错误并结束 loading', async () => {
    mockInvoke.mockRejectedValueOnce(new Error('offline'));
    await useHostStore.getState().loadHosts();
    expect(useHostStore.getState()).toMatchObject({ loading: false, error: 'Error: offline' });
  });

  it('保存时信任清理失败：结构化错误显式抛出，不静默报告为成功', async () => {
    const host = makeHost();
    mockInvoke.mockRejectedValueOnce({
      code: 'HostTrustCleanupFailed',
      detail: 'endpoint 10.0.0.1:22 的信任记录清理失败: write denied',
    });
    await expect(
      useHostStore.getState().saveHost({ ...host, authType: host.authType, password: 'secret' }),
    ).rejects.toMatchObject({ code: 'HostTrustCleanupFailed' });
    expect(useHostStore.getState().loading).toBe(false);
    expect(useHostStore.getState().error).toBeTruthy();
  });

  it('主机搜索跨名称、地址与分组名过滤且不区分大小写', () => {
    const hosts = [
      makeHost({ group: '' }),
      makeHost({ id: 'host-2', name: 'staging', host: '10.0.0.9', username: 'deploy', group: '' }),
      makeHost({ id: 'host-3', name: 'db', host: '10.0.0.10', group: 'eu-west' }),
    ];
    expect(filterHosts(hosts, '')).toHaveLength(3);
    expect(filterHosts(hosts, 'PROD')).toEqual([hosts[0]]);
    expect(filterHosts(hosts, '10.0.0.9')).toEqual([hosts[1]]);
    expect(filterHosts(hosts, 'EU')).toEqual([hosts[2]]);
    expect(filterHosts(hosts, '   ')).toHaveLength(3);
    expect(filterHosts(hosts, 'no-such-host')).toEqual([]);
  });

  it('主机搜索词与选中状态可切换', () => {
    useHostStore.getState().setSearchQuery('staging');
    expect(useHostStore.getState().searchQuery).toBe('staging');
    useHostStore.getState().selectHost('host-2');
    expect(useHostStore.getState().selectedHostId).toBe('host-2');
    useHostStore.getState().selectHost(null);
    expect(useHostStore.getState().selectedHostId).toBeNull();
  });

  it('主机分组：组名排序、未分组排最后', () => {
    const hosts = [
      makeHost({ id: 'h1', group: '' }),
      makeHost({ id: 'h2', name: 'b-host', group: 'beta' }),
      makeHost({ id: 'h3', name: 'a-host', group: 'alpha' }),
      makeHost({ id: 'h4', name: 'z-host', group: '' }),
    ];
    const groups = groupHosts(hosts);
    expect(groups.map((group) => group.name)).toEqual(['alpha', 'beta', '']);
    expect(groups[2].hosts.map((host) => host.id)).toEqual(['h1', 'h4']);
    expect(groups[0].hosts[0].id).toBe('h3');
  });

  it('全部已分组时无未分组头', () => {
    const groups = groupHosts([makeHost({ group: 'alpha' }), makeHost({ id: 'h2', group: 'beta' })]);
    expect(groups.map((group) => group.name)).toEqual(['alpha', 'beta']);
  });

  it('分组折叠状态可切换并写入本地存储', () => {
    localStorage.removeItem('collapsed-groups');
    useLayoutStore.setState({ collapsedGroups: [] });
    useLayoutStore.getState().toggleGroupCollapsed('alpha');
    expect(useLayoutStore.getState().collapsedGroups).toEqual(['alpha']);
    expect(readCollapsedGroups()).toEqual(['alpha']);
    useLayoutStore.getState().toggleGroupCollapsed('alpha');
    expect(useLayoutStore.getState().collapsedGroups).toEqual([]);
    useLayoutStore.getState().toggleGroupCollapsed('beta');
    useLayoutStore.getState().toggleGroupCollapsed('alpha');
    expect(readCollapsedGroups()).toEqual(['beta', 'alpha']);
  });

  it('重命名分组更新组内主机并仅保存受影响主机', async () => {
    const alpha = [makeHost({ group: 'alpha' }), makeHost({ id: 'h2', name: 'b', group: 'alpha' })];
    const beta = makeHost({ id: 'h3', name: 'c', group: 'beta' });
    let current = [...alpha, beta];
    mockInvoke.mockImplementation(async (command, args) => {
      if (command === 'save_host') {
        current = current.map((h) => h.id === args.request.id ? { ...h, group: args.request.group } : h);
        return current;
      }
      return current;
    });
    useHostStore.setState({ hosts: current });
    await useHostStore.getState().renameGroup('alpha', 'prod');
    expect(mockInvoke).toHaveBeenCalledTimes(2);
    expect(useHostStore.getState().hosts.filter((h) => h.group === 'prod')).toHaveLength(2);
    expect(useHostStore.getState().hosts.find((h) => h.id === 'h3')?.group).toBe('beta');
  });

  it('重命名同名或空白名不触发保存', async () => {
    const all = [makeHost({ group: 'alpha' })];
    useHostStore.setState({ hosts: all });
    await useHostStore.getState().renameGroup('alpha', 'alpha');
    await useHostStore.getState().renameGroup('alpha', '   ');
    expect(mockInvoke).not.toHaveBeenCalled();
  });

  it('删除分组后组内主机归入未分组', async () => {
    let current = [makeHost({ group: 'alpha' }), makeHost({ id: 'h2', name: 'b', group: 'alpha' })];
    mockInvoke.mockImplementation(async (command, args) => {
      if (command === 'save_host') {
        current = current.map((h) => h.id === args.request.id ? { ...h, group: args.request.group } : h);
        return current;
      }
      return current;
    });
    useHostStore.setState({ hosts: current });
    await useHostStore.getState().deleteGroup('alpha');
    expect(useHostStore.getState().hosts.every((h) => h.group === '')).toBe(true);
    expect(mockInvoke).toHaveBeenCalledTimes(2);
  });

  it('折叠状态随分组重命名迁移、删除时移除', () => {
    useLayoutStore.setState({ collapsedGroups: ['alpha', 'beta'] });
    useLayoutStore.getState().renameCollapsedGroup('alpha', 'prod');
    expect(useLayoutStore.getState().collapsedGroups).toEqual(['prod', 'beta']);
    useLayoutStore.getState().removeCollapsedGroup('prod');
    expect(useLayoutStore.getState().collapsedGroups).toEqual(['beta']);
    expect(readCollapsedGroups()).toEqual(['beta']);
  });

  it('监视条折叠状态可切换并持久化', () => {
    localStorage.removeItem('monitor-collapsed');
    useLayoutStore.setState({ monitorCollapsed: false });
    useLayoutStore.getState().toggleMonitorCollapsed();
    expect(useLayoutStore.getState().monitorCollapsed).toBe(true);
    expect(readMonitorCollapsed()).toBe(true);
    useLayoutStore.getState().toggleMonitorCollapsed();
    expect(useLayoutStore.getState().monitorCollapsed).toBe(false);
    expect(readMonitorCollapsed()).toBe(false);
  });

  it('侧栏宽度使用默认值并限制最小值与主区域空间', () => {
    expect(useLayoutStore.getState().sidebarWidth).toBe(DEFAULT_SIDEBAR_WIDTH);
    useLayoutStore.getState().setSidebarWidth(1);
    expect(useLayoutStore.getState().sidebarWidth).toBe(MIN_SIDEBAR_WIDTH);
    useLayoutStore.setState({ sidebarWidth: 520 });
    useLayoutStore.getState().syncSidebarWidthForViewport(760);
    expect(useLayoutStore.getState().sidebarWidth).toBe(760 - MIN_MAIN_PANEL_WIDTH);
  });

  it('打开会话后设为激活并启动监控', async () => {
    const session = makeSession();
    const task = makeTaskInfo();
    mockInvoke.mockImplementation(async (command) => command === 'open_session' ? session : task);
    await useSessionStore.getState().openSession('host-1');
    expect(useSessionStore.getState().activeTabId).toBe(terminalTabId(session.sessionId));
    expect(useMonitorStore.getState().sessionTaskMap.get(session.sessionId)).toBe(task.taskId);
  });

  it('标签视图模型：打开会话建立恰好一个终端标签（会话锚点）并按打开顺序排列', async () => {
    let openCount = 0;
    mockInvoke.mockImplementation(async (command) => {
      if (command === 'open_session') {
        openCount += 1;
        return makeSession({ sessionId: `session-${openCount}` });
      }
      return makeTaskInfo();
    });
    await useSessionStore.getState().openSession('host-1');
    await useSessionStore.getState().openSession('host-1');

    const state = useSessionStore.getState();
    // 一个会话恰有一个终端标签：两会话 → 两标签，顺序与打开顺序一致（标签栏渲染源）
    expect([...state.tabs.values()].map((tab) => tab.sessionId)).toEqual(['session-1', 'session-2']);
    expect([...state.tabs.values()].every((tab) => tab.type === 'terminal')).toBe(true);
    expect(state.activeTabId).toBe(terminalTabId('session-2'));
  });

  it('锚点语义：关闭终端标签触发 close_session 完整 teardown 并清理会话与标签投影', async () => {
    useSessionStore.setState({
      sessions: new Map([['session-1', makeSession()]]),
      tabs: new Map([[terminalTabId('session-1'), makeTerminalTab()]]),
      activeTabId: terminalTabId('session-1'),
    });
    mockInvoke.mockResolvedValue(undefined);

    await useSessionStore.getState().closeTab(terminalTabId('session-1'));

    expect(mockInvoke).toHaveBeenCalledWith('close_session', { sessionId: 'session-1' });
    const state = useSessionStore.getState();
    expect(state.sessions.has('session-1')).toBe(false);
    expect(state.tabs.has(terminalTabId('session-1'))).toBe(false);
    expect(state.activeTabId).toBeNull();
  });

  it('锚点语义：关闭后台会话的终端标签不影响当前激活标签', async () => {
    useSessionStore.setState({
      sessions: new Map([
        ['session-1', makeSession()],
        ['session-2', makeSession({ sessionId: 'session-2' })],
      ]),
      tabs: new Map([
        [terminalTabId('session-1'), makeTerminalTab()],
        [terminalTabId('session-2'), makeTerminalTab({ tabId: terminalTabId('session-2'), sessionId: 'session-2' })],
      ]),
      activeTabId: terminalTabId('session-2'),
    });
    mockInvoke.mockResolvedValue(undefined);

    await useSessionStore.getState().closeTab(terminalTabId('session-1'));

    expect(mockInvoke).toHaveBeenCalledWith('close_session', { sessionId: 'session-1' });
    expect(useSessionStore.getState().tabs.has(terminalTabId('session-1'))).toBe(false);
    expect(useSessionStore.getState().activeTabId).toBe(terminalTabId('session-2'));
  });

  it('关闭未知标签为无操作：不发起后端调用也不改动状态', async () => {
    mockInvoke.mockResolvedValue(undefined);
    await useSessionStore.getState().closeTab(terminalTabId('ghost'));
    expect(mockInvoke).not.toHaveBeenCalled();
    expect(useSessionStore.getState().tabs.size).toBe(0);
    expect(useSessionStore.getState().activeTabId).toBeNull();
  });

  it('切换激活标签：setActiveTab 迁移视图选择状态到标签语义', () => {
    useSessionStore.setState({
      tabs: new Map([
        [terminalTabId('session-1'), makeTerminalTab()],
        [terminalTabId('session-2'), makeTerminalTab({ tabId: terminalTabId('session-2'), sessionId: 'session-2' })],
      ]),
      activeTabId: terminalTabId('session-1'),
    });
    useSessionStore.getState().setActiveTab(terminalTabId('session-2'));
    expect(useSessionStore.getState().activeTabId).toBe(terminalTabId('session-2'));
  });

  it('监控启动失败不阻断会话打开', async () => {
    mockInvoke.mockImplementation(async (command) => {
      if (command === 'open_session') return makeSession();
      throw new Error('monitor failed');
    });
    await expect(useSessionStore.getState().openSession('host-1')).resolves.toMatchObject({ sessionId: 'session-1' });
  });

  it('连接阶段与失败错误按 sessionId 更新且互不覆盖', async () => {
    let openCount = 0;
    mockInvoke.mockImplementation(async (command) => {
      if (command === 'open_session') {
        openCount += 1;
        return makeSession({ sessionId: `session-${openCount}` });
      }
      return makeTaskInfo();
    });
    await useSessionStore.getState().openSession('host-1');
    await useSessionStore.getState().openSession('host-1');
    expect(useSessionStore.getState().connections.get('session-1')).toEqual({ phase: null, error: null });
    const cleanup = await useSessionStore.getState().initListeners();

    emitMockEvent('session:progress', { sessionId: 'session-1', phase: ConnectionPhase.SshHandshake, timestamp: Date.now() });
    emitMockEvent('session:progress', { sessionId: 'session-2', phase: ConnectionPhase.Authenticating, timestamp: Date.now() });
    expect(useSessionStore.getState().connections.get('session-1')).toEqual({ phase: ConnectionPhase.SshHandshake, error: null });
    expect(useSessionStore.getState().connections.get('session-2')).toEqual({ phase: ConnectionPhase.Authenticating, error: null });

    emitMockEvent('session:status', { sessionId: 'session-2', status: SessionStatus.AuthFailed, error: null });
    expect(useSessionStore.getState().connections.get('session-2')).toEqual({ phase: null, error: null });
    expect(useSessionStore.getState().connections.get('session-1')).toEqual({ phase: ConnectionPhase.SshHandshake, error: null });

    emitMockEvent('session:status', { sessionId: 'session-1', status: SessionStatus.Connected, error: null });
    expect(useSessionStore.getState().connections.has('session-1')).toBe(false);
    expect(useSessionStore.getState().sessions.get('session-2')?.status).toBe(SessionStatus.AuthFailed);
    cleanup();
  });

  it('连接失败保留结构化错误供所属标签渲染', async () => {
    useSessionStore.setState({ sessions: new Map([['session-1', makeSession()]]) });
    useSessionStore.getState().applySessionStatus({
      sessionId: 'session-1', status: SessionStatus.Error, error: { code: 'SshConnectionError', detail: 'connection refused' },
    });
    expect(useSessionStore.getState().connections.get('session-1')).toEqual({
      phase: null, error: { code: 'SshConnectionError', detail: 'connection refused' },
    });
  });

  it('断开的会话清除连接投影，不再残留不可见状态', () => {
    useSessionStore.setState({
      sessions: new Map([['session-1', makeSession({ status: SessionStatus.Connected })]]),
      connections: new Map([['session-1', { phase: ConnectionPhase.SshHandshake, error: null }]]),
    });
    useSessionStore.getState().applySessionStatus({ sessionId: 'session-1', status: SessionStatus.Disconnected, error: null });
    expect(useSessionStore.getState().connections.has('session-1')).toBe(false);
  });

  it('连接文案在渲染时生成且随语言切换即时生效', () => {
    const session = makeSession();
    const failed = makeSession({ status: SessionStatus.Error });
    const connecting = { phase: ConnectionPhase.SshHandshake, error: null };
    const failedConnection = { phase: null, error: { code: 'SshConnectionError', detail: 'connection refused' } };
    expect(connectionLabel(session, connecting, 'zh-CN')).toContain('SSH 握手');
    expect(connectionLabel(session, connecting, 'en-US')).toContain('SSH handshake');
    expect(connectionLabel(session, undefined, 'zh-CN')).toContain('正在连接');
    expect(connectionLabel(failed, failedConnection, 'zh-CN')).toContain('SSH 连接失败');
    expect(connectionLabel(failed, failedConnection, 'en-US')).toContain('SSH connection failed');
  });

  it('打开会话时初始化文件传输且连接成功事件不重复请求目录', async () => {
    mockInvoke.mockImplementation(async (command) => {
      if (command === 'open_session') return makeSession();
      if (command === 'start_monitoring') return makeTaskInfo();
      if (command === 'sftp_list_dir') return [];
      return undefined;
    });
    await useSessionStore.getState().openSession('host-1');
    expect(mockInvoke).toHaveBeenCalledWith('sftp_list_dir', { sessionId: 'session-1', path: '/' });
    const cleanup = await useSessionStore.getState().initListeners();
    mockInvoke.mockClear();

    emitMockEvent('session:status', {
      sessionId: 'session-1', status: SessionStatus.Connected, error: null,
    });

    expect(mockInvoke).not.toHaveBeenCalledWith('sftp_list_dir', expect.anything());
    expect(mockInvoke).not.toHaveBeenCalledWith('sync_session_status', expect.anything());
    cleanup();
  });

  it('open_session 返回前到达连接成功事件时仍初始化文件传输', async () => {
    mockInvoke.mockImplementation(async (command) => {
      if (command === 'open_session') {
        emitMockEvent('session:status', {
          sessionId: 'session-1', status: SessionStatus.Connected, error: null,
        });
        return makeSession();
      }
      if (command === 'start_monitoring') return makeTaskInfo();
      if (command === 'sftp_list_dir') return [makeRemoteEntry()];
      return undefined;
    });
    const cleanup = await useSessionStore.getState().initListeners();

    await useSessionStore.getState().openSession('host-1');

    await vi.waitFor(() => {
      expect(useSftpStore.getState().getState('session-1')?.entries).toHaveLength(1);
    });
    cleanup();
  });

  it('前端不会用 watchdog 覆盖后端 Session 状态', async () => {
    vi.useFakeTimers();
    mockInvoke.mockImplementation(async (command) => command === 'open_session' ? makeSession() : makeTaskInfo());
    await useSessionStore.getState().openSession('host-1');
    vi.advanceTimersByTime(15_000);
    expect(useSessionStore.getState().sessions.get('session-1')?.status).toBe(SessionStatus.Connecting);
    vi.useRealTimers();
  });

  it('首个进度事件先于 open_session 返回到达时不丢弃，投影建立后回放阶段', async () => {
    // 复现 IPC 竞态：worker 在 open_session 返回前就发出 LoadingCredentials 进度，
    // 事件先到、投影后建。丢弃会让标签永远显示通用“正在连接”而非卡点提示。
    const cleanup = await useSessionStore.getState().initListeners();
    emitMockEvent('session:progress', { sessionId: 'session-1', phase: ConnectionPhase.LoadingCredentials, timestamp: 1 });
    expect(useSessionStore.getState().connections.has('session-1')).toBe(false);

    mockInvoke.mockImplementation(async (command) => command === 'open_session' ? makeSession() : makeTaskInfo());
    await useSessionStore.getState().openSession('host-1');

    expect(useSessionStore.getState().connections.get('session-1')?.phase).toBe(ConnectionPhase.LoadingCredentials);
    cleanup();
  });

  it('关闭活动会话只调用后端 teardown 并清理前端 projection', async () => {
    const task = makeTaskInfo();
    useSessionStore.setState({
      sessions: new Map([['session-1', makeSession()]]),
      tabs: new Map([[terminalTabId('session-1'), makeTerminalTab()]]),
      activeTabId: terminalTabId('session-1'),
      connections: new Map([['session-1', { phase: ConnectionPhase.SshHandshake, error: null }]]),
    });
    useMonitorStore.setState({
      sessionTaskMap: new Map([['session-1', task.taskId]]),
      tasks: new Map([[task.taskId, task]]),
      snapshots: new Map([['session-1', makeSnapshot()]]),
      selectedInterfaces: new Map([['session-1', 'eth0']]),
    });
    mockInvoke.mockResolvedValue(undefined);
    await useSessionStore.getState().closeSession('session-1');

    expect(useSessionStore.getState().activeTabId).toBeNull();
    expect(mockInvoke).toHaveBeenCalledWith('close_session', { sessionId: 'session-1' });
    expect(useSessionStore.getState().connections.has('session-1')).toBe(false);
    expect(useSessionStore.getState().tabs.has(terminalTabId('session-1'))).toBe(false);
    expect(mockInvoke).not.toHaveBeenCalledWith('stop_monitoring', expect.anything());
    expect(useMonitorStore.getState().sessionTaskMap.has('session-1')).toBe(false);
    expect(useMonitorStore.getState().tasks.has(task.taskId)).toBe(false);
    expect(useMonitorStore.getState().snapshots.has('session-1')).toBe(false);
    expect(useMonitorStore.getState().selectedInterfaces.has('session-1')).toBe(false);
  });

  it('连接失败后端已自动 teardown：close_session 返回 SessionNotFound 时仍清理投影，标签始终可关闭', async () => {
    // 复现凭据不存在场景：worker 退出触发 TerminalExitGuard 清理会话，
    // 前端投影保留 Error 状态与覆盖层，用户点击关闭时后端返回 SessionNotFound
    useSessionStore.setState({
      sessions: new Map([['session-1', makeSession({ status: SessionStatus.Error })]]),
      tabs: new Map([[terminalTabId('session-1'), makeTerminalTab()]]),
      activeTabId: terminalTabId('session-1'),
      connections: new Map([['session-1', {
        phase: null,
        error: { code: 'CredentialNotFound', detail: '凭据不存在: titanssh-xxx-password' },
      }]]),
    });
    mockInvoke.mockRejectedValueOnce({ code: 'SessionNotFound', detail: 'Session not found: session-1' });

    // 关闭视图是本地操作：不得因后端会话已消失而抛出或残留投影
    await expect(useSessionStore.getState().closeSession('session-1')).resolves.toBeUndefined();

    expect(useSessionStore.getState().sessions.has('session-1')).toBe(false);
    expect(useSessionStore.getState().connections.has('session-1')).toBe(false);
    expect(useSessionStore.getState().activeTabId).toBeNull();
  });

  it('用户路径回归：连接失败后关闭标签（closeTab）在 close_session 返回 SessionNotFound 时仍移除标签', async () => {
    // v0.1.5 实测缺陷：TCP 建连失败（如 No route to host）→ 终端 worker 退出触发
    // TerminalExitGuard 后端 teardown → 错误覆盖层仅剩关闭标签操作；用户点击关闭时
    // 后端返回 SessionNotFound，关闭是本地视图操作，不得被后端会话状态阻塞。
    // 入口必须是 closeTab（覆盖层按钮 → HomePage → closeTab），覆盖完整包装链。
    useSessionStore.setState({
      sessions: new Map([['session-1', makeSession({ status: SessionStatus.Error })]]),
      tabs: new Map([[terminalTabId('session-1'), makeTerminalTab()]]),
      activeTabId: terminalTabId('session-1'),
      connections: new Map([['session-1', {
        phase: null,
        error: { code: 'SshConnectionError', detail: '连接失败: 在 10000ms 预算内已尝试 1 个地址，最后错误: No route to host (os error 65)' },
      }]]),
    });
    mockInvoke.mockRejectedValueOnce({ code: 'SessionNotFound', detail: 'Session not found: session-1' });

    await expect(useSessionStore.getState().closeTab(terminalTabId('session-1'))).resolves.toBeUndefined();

    expect(mockInvoke).toHaveBeenCalledWith('close_session', { sessionId: 'session-1' });
    expect(useSessionStore.getState().sessions.has('session-1')).toBe(false);
    expect(useSessionStore.getState().tabs.has(terminalTabId('session-1'))).toBe(false);
    expect(useSessionStore.getState().activeTabId).toBeNull();
  });

  it('后端撤销事件撤下对应确认卡，且不误删已被新 challenge 取代的投影', async () => {
    const oldChallenge = {
      challengeId: 'challenge-old', sessionId: 'session-1', host: '10.0.0.8', port: 22,
      keyAlgorithm: 'ssh-ed25519', fingerprint: 'SHA256:old', timestamp: 1_710_000_000_000,
    };
    const cleanup = await useSessionStore.getState().initListeners();

    // 撤销事件到达：撤下匹配的确认卡
    emitMockEvent('host-identity:challenge', oldChallenge);
    emitMockEvent('host-identity:challenge-dismissed', { challengeId: 'challenge-old', sessionId: 'session-1' });
    expect(useSessionStore.getState().hostKeyChallenges.has('session-1')).toBe(false);

    // 新 challenge 取代后，旧 challenge 的迟到撤销事件不得误删新投影
    const newChallenge = { ...oldChallenge, challengeId: 'challenge-new', fingerprint: 'SHA256:new' };
    emitMockEvent('host-identity:challenge', newChallenge);
    emitMockEvent('host-identity:challenge-dismissed', { challengeId: 'challenge-old', sessionId: 'session-1' });
    expect(useSessionStore.getState().hostKeyChallenges.get('session-1')).toEqual(newChallenge);

    cleanup();
  });

  it('主机身份确认事件按 sessionId 投影，接受后调用后端并清除确认卡', async () => {
    const challenge = {
      challengeId: 'challenge-1', sessionId: 'session-1', host: '10.0.0.8', port: 22,
      keyAlgorithm: 'ssh-ed25519', fingerprint: 'SHA256:abc', timestamp: 1_710_000_000_000,
    };
    mockInvoke.mockResolvedValue(undefined);
    const cleanup = await useSessionStore.getState().initListeners();
    emitMockEvent('host-identity:challenge', challenge);
    expect(useSessionStore.getState().hostKeyChallenges.get('session-1')).toEqual(challenge);

    await useSessionStore.getState().acceptHostIdentity('session-1');
    expect(mockInvoke).toHaveBeenCalledWith('accept_host_identity', { challengeId: 'challenge-1' });
    expect(useSessionStore.getState().hostKeyChallenges.has('session-1')).toBe(false);
    cleanup();
  });

  it('拒绝主机身份调用后端拒绝，后端已 teardown 不重复 close_session，并清理会话与 SFTP/监控投影', async () => {
    const challenge = {
      challengeId: 'challenge-2', sessionId: 'session-1', host: '10.0.0.8', port: 22,
      keyAlgorithm: 'ssh-ed25519', fingerprint: 'SHA256:abc', timestamp: 1_710_000_000_000,
    };
    useSessionStore.setState({
      sessions: new Map([['session-1', makeSession()]]),
      tabs: new Map([[terminalTabId('session-1'), makeTerminalTab()]]),
      activeTabId: terminalTabId('session-1'),
      hostKeyChallenges: new Map([['session-1', challenge]]),
    });
    mockInvoke.mockResolvedValue(undefined);

    await useSessionStore.getState().rejectHostIdentity('session-1');

    expect(mockInvoke).toHaveBeenCalledWith('reject_host_identity', { challengeId: 'challenge-2' });
    // 后端在拒绝命令内完成 teardown，前端不得重复 close_session（性能规则：无冗余 invoke）
    expect(mockInvoke).not.toHaveBeenCalledWith('close_session', { sessionId: 'session-1' });
    expect(useSessionStore.getState().sessions.has('session-1')).toBe(false);
    expect(useSessionStore.getState().hostKeyChallenges.has('session-1')).toBe(false);
    expect(useSessionStore.getState().activeTabId).toBeNull();
    expect(useSftpStore.getState().getState('session-1')).toBeUndefined();
    expect(useMonitorStore.getState().snapshots.has('session-1')).toBe(false);
  });

  it('接受主机身份时 challenge 已不存在（重复操作）仅撤下过期确认卡，不抛错', async () => {
    const challenge = {
      challengeId: 'challenge-gone', sessionId: 'session-1', host: '10.0.0.8', port: 22,
      keyAlgorithm: 'ssh-ed25519', fingerprint: 'SHA256:abc', timestamp: 1_710_000_000_000,
    };
    useSessionStore.setState({
      sessions: new Map([['session-1', makeSession()]]),
      hostKeyChallenges: new Map([['session-1', challenge]]),
    });
    mockInvoke.mockRejectedValue({ code: 'HostKeyChallengeNotFound', detail: 'challenge-gone' });

    await useSessionStore.getState().acceptHostIdentity('session-1');

    expect(useSessionStore.getState().hostKeyChallenges.has('session-1')).toBe(false);
    expect(useSessionStore.getState().sessions.has('session-1')).toBe(true);
  });

  it('接受主机身份其他错误保留确认卡，避免掩盖未决决定', async () => {
    const challenge = {
      challengeId: 'challenge-err', sessionId: 'session-1', host: '10.0.0.8', port: 22,
      keyAlgorithm: 'ssh-ed25519', fingerprint: 'SHA256:abc', timestamp: 1_710_000_000_000,
    };
    useSessionStore.setState({ hostKeyChallenges: new Map([['session-1', challenge]]) });
    mockInvoke.mockRejectedValue({ code: 'SshProtocolError', detail: 'boom' });

    await useSessionStore.getState().acceptHostIdentity('session-1');

    expect(useSessionStore.getState().hostKeyChallenges.has('session-1')).toBe(true);
  });

  it('拒绝主机身份时 challenge 已不存在仅撤下确认卡，不误杀仍存活的会话投影', async () => {
    const challenge = {
      challengeId: 'challenge-gone', sessionId: 'session-1', host: '10.0.0.8', port: 22,
      keyAlgorithm: 'ssh-ed25519', fingerprint: 'SHA256:abc', timestamp: 1_710_000_000_000,
    };
    useSessionStore.setState({
      sessions: new Map([['session-1', makeSession()]]),
      tabs: new Map([[terminalTabId('session-1'), makeTerminalTab()]]),
      activeTabId: terminalTabId('session-1'),
      hostKeyChallenges: new Map([['session-1', challenge]]),
    });
    mockInvoke.mockRejectedValue({ code: 'HostKeyChallengeNotFound', detail: 'challenge-gone' });

    await useSessionStore.getState().rejectHostIdentity('session-1');

    expect(useSessionStore.getState().hostKeyChallenges.has('session-1')).toBe(false);
    expect(useSessionStore.getState().sessions.has('session-1')).toBe(true);
    expect(useSessionStore.getState().activeTabId).toBe(terminalTabId('session-1'));
  });

  it('接受并保存成功：调用后端命令并清除确认卡', async () => {
    const challenge = {
      challengeId: 'challenge-save', sessionId: 'session-1', host: '10.0.0.8', port: 22,
      keyAlgorithm: 'ssh-ed25519', fingerprint: 'SHA256:abc', timestamp: 1_710_000_000_000,
    };
    useSessionStore.setState({ hostKeyChallenges: new Map([['session-1', challenge]]) });
    mockInvoke.mockResolvedValue(undefined);

    await useSessionStore.getState().acceptAndSaveHostIdentity('session-1');

    expect(mockInvoke).toHaveBeenCalledWith('accept_and_save_host_identity', { challengeId: 'challenge-save' });
    expect(useSessionStore.getState().hostKeyChallenges.has('session-1')).toBe(false);
  });

  it('接受并保存失败：保持确认卡并记录结构化错误，不自动降级为临时信任', async () => {
    const challenge = {
      challengeId: 'challenge-fail', sessionId: 'session-1', host: '10.0.0.8', port: 22,
      keyAlgorithm: 'ssh-ed25519', fingerprint: 'SHA256:abc', timestamp: 1_710_000_000_000,
    };
    useSessionStore.setState({ hostKeyChallenges: new Map([['session-1', challenge]]) });
    mockInvoke.mockRejectedValue({ code: 'HostKeySaveFailed', detail: 'write denied' });

    await useSessionStore.getState().acceptAndSaveHostIdentity('session-1');

    // 未调用 accept_host_identity：失败绝不静默降级为临时信任
    expect(mockInvoke).not.toHaveBeenCalledWith('accept_host_identity', expect.anything());
    expect(useSessionStore.getState().hostKeyChallenges.get('session-1')).toEqual(challenge);
    expect(useSessionStore.getState().hostKeySaveErrors.get('session-1')).toEqual({ code: 'HostKeySaveFailed', detail: 'write denied' });

    // 用户改选仅本次接受：清除错误投影并正常解决
    mockInvoke.mockResolvedValue(undefined);
    await useSessionStore.getState().acceptHostIdentity('session-1');
    expect(useSessionStore.getState().hostKeyChallenges.has('session-1')).toBe(false);
    expect(useSessionStore.getState().hostKeySaveErrors.has('session-1')).toBe(false);
  });

  it('接受并保存时 challenge 已不存在（重复操作）仅撤下过期确认卡', async () => {
    const challenge = {
      challengeId: 'challenge-save-gone', sessionId: 'session-1', host: '10.0.0.8', port: 22,
      keyAlgorithm: 'ssh-ed25519', fingerprint: 'SHA256:abc', timestamp: 1_710_000_000_000,
    };
    useSessionStore.setState({ hostKeyChallenges: new Map([['session-1', challenge]]) });
    mockInvoke.mockRejectedValue({ code: 'HostKeyChallengeNotFound', detail: 'challenge-save-gone' });

    await useSessionStore.getState().acceptAndSaveHostIdentity('session-1');

    expect(useSessionStore.getState().hostKeyChallenges.has('session-1')).toBe(false);
    expect(useSessionStore.getState().hostKeySaveErrors.has('session-1')).toBe(false);
  });

  it('无确认卡时接受并保存为无操作：不发起 invoke 也不写入错误', async () => {
    useSessionStore.setState({ hostKeyChallenges: new Map() });

    await useSessionStore.getState().acceptAndSaveHostIdentity('session-1');

    expect(mockInvoke).not.toHaveBeenCalled();
    expect(useSessionStore.getState().hostKeySaveErrors.has('session-1')).toBe(false);
  });

  it('Changed challenge 事件按 sessionId 投影并携带 kind、旧记录与新呈现信息，替换走同一保存契约', async () => {
    const challenge = {
      challengeId: 'challenge-changed', sessionId: 'session-1', host: '10.0.0.8', port: 22,
      kind: 'Changed' as const,
      keyAlgorithm: 'ssh-rsa', fingerprint: 'SHA256:newfp',
      storedAlgorithm: 'ssh-ed25519', storedFingerprint: 'SHA256:oldfp',
      timestamp: 1_710_000_000_000,
    };
    mockInvoke.mockResolvedValue(undefined);
    const cleanup = await useSessionStore.getState().initListeners();
    emitMockEvent('host-identity:challenge', challenge);
    expect(useSessionStore.getState().hostKeyChallenges.get('session-1')).toEqual(challenge);
    cleanup();

    // 替换记录与接受并保存共用同一后端契约（替换由确认卡二次确认把关）
    await useSessionStore.getState().acceptAndSaveHostIdentity('session-1');
    expect(mockInvoke).toHaveBeenCalledWith('accept_and_save_host_identity', { challengeId: 'challenge-changed' });
    expect(useSessionStore.getState().hostKeyChallenges.has('session-1')).toBe(false);
  });

  it('替换失败（Changed challenge）保持确认卡与结构化错误，可改选仅本次接受或拒绝', async () => {
    const challenge = {
      challengeId: 'challenge-replace-fail', sessionId: 'session-1', host: '10.0.0.8', port: 22,
      kind: 'Changed' as const,
      keyAlgorithm: 'ssh-rsa', fingerprint: 'SHA256:newfp',
      storedAlgorithm: 'ssh-ed25519', storedFingerprint: 'SHA256:oldfp',
      timestamp: 1_710_000_000_000,
    };
    useSessionStore.setState({ hostKeyChallenges: new Map([['session-1', challenge]]) });
    mockInvoke.mockRejectedValue({ code: 'HostKeySaveFailed', detail: 'write denied' });

    await useSessionStore.getState().acceptAndSaveHostIdentity('session-1');

    // 替换写入失败：未降级为临时信任，challenge 保持未决
    expect(mockInvoke).not.toHaveBeenCalledWith('accept_host_identity', expect.anything());
    expect(useSessionStore.getState().hostKeyChallenges.get('session-1')).toEqual(challenge);
    expect(useSessionStore.getState().hostKeySaveErrors.get('session-1')).toEqual({ code: 'HostKeySaveFailed', detail: 'write denied' });

    // 用户明确改选仅本次接受：正常解决并清除错误投影
    mockInvoke.mockResolvedValue(undefined);
    await useSessionStore.getState().acceptHostIdentity('session-1');
    expect(useSessionStore.getState().hostKeyChallenges.has('session-1')).toBe(false);
    expect(useSessionStore.getState().hostKeySaveErrors.has('session-1')).toBe(false);
  });

  it('会话状态进入 Connected（跨 Session 保存放行）时清理确认卡与保存错误投影', async () => {
    const challenge = {
      challengeId: 'challenge-cross', sessionId: 'session-1', host: '10.0.0.8', port: 22,
      keyAlgorithm: 'ssh-ed25519', fingerprint: 'SHA256:abc', timestamp: 1_710_000_000_000,
    };
    useSessionStore.setState({
      sessions: new Map([['session-1', makeSession()]]),
      hostKeyChallenges: new Map([['session-1', challenge]]),
      hostKeySaveErrors: new Map([['session-1', { code: 'HostKeySaveFailed', detail: 'write denied' }]]),
    });
    const cleanup = await useSessionStore.getState().initListeners();

    emitMockEvent('session:status', { sessionId: 'session-1', status: SessionStatus.Connected, error: null });

    expect(useSessionStore.getState().hostKeyChallenges.has('session-1')).toBe(false);
    expect(useSessionStore.getState().hostKeySaveErrors.has('session-1')).toBe(false);
    cleanup();
  });

  it('非 Connected 状态不隐式清理确认卡：未决 challenge 保持可见可决', async () => {
    const challenge = {
      challengeId: 'challenge-keep', sessionId: 'session-1', host: '10.0.0.8', port: 22,
      keyAlgorithm: 'ssh-ed25519', fingerprint: 'SHA256:abc', timestamp: 1_710_000_000_000,
    };
    useSessionStore.setState({
      sessions: new Map([['session-1', makeSession()]]),
      hostKeyChallenges: new Map([['session-1', challenge]]),
    });
    const cleanup = await useSessionStore.getState().initListeners();

    emitMockEvent('session:status', { sessionId: 'session-1', status: SessionStatus.Timeout, error: null });

    expect(useSessionStore.getState().hostKeyChallenges.get('session-1')).toEqual(challenge);
    cleanup();
  });

  it('新 challenge 到达时清除此前的保存错误投影', async () => {
    const previous = {
      challengeId: 'challenge-old', sessionId: 'session-1', host: '10.0.0.8', port: 22,
      keyAlgorithm: 'ssh-ed25519', fingerprint: 'SHA256:old', timestamp: 1_710_000_000_000,
    };
    const next = { ...previous, challengeId: 'challenge-new', fingerprint: 'SHA256:new' };
    useSessionStore.setState({
      hostKeyChallenges: new Map([['session-1', previous]]),
      hostKeySaveErrors: new Map([['session-1', { code: 'HostKeySaveFailed', detail: 'write denied' }]]),
    });
    const cleanup = await useSessionStore.getState().initListeners();
    emitMockEvent('host-identity:challenge', next);
    expect(useSessionStore.getState().hostKeyChallenges.get('session-1')).toEqual(next);
    expect(useSessionStore.getState().hostKeySaveErrors.has('session-1')).toBe(false);
    cleanup();
  });

  it('监控事件按 sessionId 更新快照并流转任务状态', async () => {
    const task = makeTaskInfo();
    useMonitorStore.setState({ tasks: new Map([[task.taskId, task]]) });
    const cleanup = await useMonitorStore.getState().initListeners();
    emitMockEvent('monitor:snapshot', makeSnapshot());
    emitMockEvent('task:status', { taskId: task.taskId, status: TaskStatus.Done });
    expect(useMonitorStore.getState().snapshots.get('session-1')?.cpuUsage).toBe(21.5);
    expect(useMonitorStore.getState().tasks.get(task.taskId)?.status).toBe(TaskStatus.Done);
    cleanup();
  });

  it('网卡选择首次默认第一张、可切换且后台会话彼此隔离', () => {
    useMonitorStore.getState().applySnapshot(makeSnapshot({ network: {
      available: true,
      interfaces: [
        { name: 'eth0', receiveBytesPerSecond: 1024, transmitBytesPerSecond: 512 },
        { name: 'eth1', receiveBytesPerSecond: 2048, transmitBytesPerSecond: 1024 },
      ],
    } }));
    expect(useMonitorStore.getState().selectedInterfaces.get('session-1')).toBe('eth0');

    useMonitorStore.getState().selectNetworkInterface('session-1', 'eth1');
    useMonitorStore.getState().applySnapshot(makeSnapshot({ sessionId: 'session-2', network: {
      available: true,
      interfaces: [{ name: 'ens5', receiveBytesPerSecond: 4096, transmitBytesPerSecond: 2048 }],
    } }));

    expect(useMonitorStore.getState().selectedInterfaces.get('session-1')).toBe('eth1');
    expect(useMonitorStore.getState().selectedInterfaces.get('session-2')).toBe('ens5');
  });

  it('网卡选择在不可用时保留，接口消失或空列表时按规则更新', () => {
    const available = (interfaces: Array<{ name: string; receiveBytesPerSecond: number | null; transmitBytesPerSecond: number | null }>) =>
      makeSnapshot({ network: { available: true, interfaces } });
    useMonitorStore.getState().applySnapshot(available([
      { name: 'eth0', receiveBytesPerSecond: 1, transmitBytesPerSecond: 1 },
      { name: 'eth1', receiveBytesPerSecond: 2, transmitBytesPerSecond: 2 },
    ]));
    useMonitorStore.getState().selectNetworkInterface('session-1', 'eth1');

    useMonitorStore.getState().applySnapshot(makeSnapshot({ network: { available: false, interfaces: [] } }));
    expect(useMonitorStore.getState().selectedInterfaces.get('session-1')).toBe('eth1');
    useMonitorStore.getState().applySnapshot(available([
      { name: 'eth1', receiveBytesPerSecond: 2, transmitBytesPerSecond: 2 },
      { name: 'eth2', receiveBytesPerSecond: 3, transmitBytesPerSecond: 3 },
    ]));
    expect(useMonitorStore.getState().selectedInterfaces.get('session-1')).toBe('eth1');

    useMonitorStore.getState().applySnapshot(available([{ name: 'eth2', receiveBytesPerSecond: 3, transmitBytesPerSecond: 3 }]));
    expect(useMonitorStore.getState().selectedInterfaces.get('session-1')).toBe('eth2');
    useMonitorStore.getState().applySnapshot(available([]));
    expect(useMonitorStore.getState().selectedInterfaces.has('session-1')).toBe(false);
    useMonitorStore.getState().applySnapshot(available([{ name: 'eth3', receiveBytesPerSecond: 4, transmitBytesPerSecond: 4 }]));
    expect(useMonitorStore.getState().selectedInterfaces.get('session-1')).toBe('eth3');

    useMonitorStore.getState().clearSession('session-1');
    expect(useMonitorStore.getState().selectedInterfaces.has('session-1')).toBe(false);
  });

  it('所选网卡趋势按真实 60 秒窗口淘汰，后台 Session 不互相混入', () => {
    const snapshot = (timestamp: number, sessionId = 'session-1', receiveBytesPerSecond = 1) => makeSnapshot({
      sessionId,
      timestamp,
      network: { available: true, interfaces: [
        { name: 'eth0', receiveBytesPerSecond, transmitBytesPerSecond: receiveBytesPerSecond * 2 },
        { name: 'eth1', receiveBytesPerSecond: receiveBytesPerSecond * 3, transmitBytesPerSecond: receiveBytesPerSecond * 4 },
      ] },
    });
    useMonitorStore.getState().applySnapshot(snapshot(0));
    useMonitorStore.getState().applySnapshot(snapshot(30_000, 'session-2', 8));
    useMonitorStore.getState().applySnapshot(snapshot(30_000, 'session-1', 2));
    useMonitorStore.getState().applySnapshot(snapshot(60_001, 'session-1', 3));

    expect(useMonitorStore.getState().networkTrends.get('session-1')?.map((sample) => sample.timestamp)).toEqual([30_000, 60_001]);
    expect(useMonitorStore.getState().networkTrends.get('session-2')?.map((sample) => sample.receiveBytesPerSecond)).toEqual([8]);
    useMonitorStore.getState().selectNetworkInterface('session-1', 'eth1');
    expect(useMonitorStore.getState().networkTrends.has('session-1')).toBe(false);
    useMonitorStore.getState().applySnapshot(snapshot(61_000, 'session-1', 4));
    expect(useMonitorStore.getState().networkTrends.get('session-1')).toEqual([
      { timestamp: 61_000, receiveBytesPerSecond: 12, transmitBytesPerSecond: 16 },
    ]);
  });

  it('网络不可用和未知速率写入趋势缺口，恢复后继续并在关闭时清理', () => {
    useMonitorStore.getState().applySnapshot(makeSnapshot({ timestamp: 1_000, network: {
      available: true,
      interfaces: [{ name: 'eth0', receiveBytesPerSecond: 1, transmitBytesPerSecond: 2 }],
    } }));
    useMonitorStore.getState().applySnapshot(makeSnapshot({ timestamp: 2_000, network: { available: false, interfaces: [] } }));
    useMonitorStore.getState().applySnapshot(makeSnapshot({ timestamp: 3_000, network: {
      available: true,
      interfaces: [{ name: 'eth0', receiveBytesPerSecond: null, transmitBytesPerSecond: null }],
    } }));
    useMonitorStore.getState().applySnapshot(makeSnapshot({ timestamp: 4_000, network: {
      available: true,
      interfaces: [{ name: 'eth0', receiveBytesPerSecond: 4, transmitBytesPerSecond: 8 }],
    } }));

    expect(useMonitorStore.getState().networkTrends.get('session-1')).toEqual([
      { timestamp: 1_000, receiveBytesPerSecond: 1, transmitBytesPerSecond: 2 },
      { timestamp: 2_000, receiveBytesPerSecond: null, transmitBytesPerSecond: null },
      { timestamp: 3_000, receiveBytesPerSecond: null, transmitBytesPerSecond: null },
      { timestamp: 4_000, receiveBytesPerSecond: 4, transmitBytesPerSecond: 8 },
    ]);
    useMonitorStore.getState().clearSession('session-1');
    expect(useMonitorStore.getState().networkTrends.has('session-1')).toBe(false);
  });

  it('SFTP 目录成功与失败分别更新 entries 和 error', async () => {
    mockInvoke.mockResolvedValueOnce([makeRemoteEntry()]).mockRejectedValueOnce(new Error('denied'));
    await useSftpStore.getState().listDir('session-1', '/var/log');
    expect(useSftpStore.getState().getState('session-1')?.entries).toHaveLength(1);
    await useSftpStore.getState().listDir('session-1', '/root');
    expect(useSftpStore.getState().getState('session-1')?.error?.detail).toContain('denied');
  });

  it('SFTP 多会话状态互不影响', async () => {
    mockInvoke.mockResolvedValueOnce([makeRemoteEntry()]).mockResolvedValueOnce([]);
    useSftpStore.getState().ensureState('a');
    useSftpStore.getState().ensureState('b');
    await useSftpStore.getState().listDir('a', '/a');
    await useSftpStore.getState().listDir('b', '/b');
    expect(useSftpStore.getState().getState('a')?.entries).toHaveLength(1);
    expect(useSftpStore.getState().getState('b')?.entries).toHaveLength(0);
  });

  it('SFTP 同会话乱序目录响应仅最新请求可更新路径、条目、loading 与 error', async () => {
    let resolveOld!: (entries: RemoteEntry[]) => void;
    let resolveNew!: (entries: RemoteEntry[]) => void;
    mockInvoke
      .mockImplementationOnce(() => new Promise<RemoteEntry[]>((resolve) => { resolveOld = resolve; }))
      .mockImplementationOnce(() => new Promise<RemoteEntry[]>((resolve) => { resolveNew = resolve; }));

    const oldRequest = useSftpStore.getState().listDir('session-1', '/old');
    const newRequest = useSftpStore.getState().listDir('session-1', '/new');
    // 新请求先完成
    resolveNew([makeRemoteEntry({ name: 'new.txt', path: '/new/new.txt' })]);
    await newRequest;
    // 旧请求后完成，不得让投影倒退
    resolveOld([makeRemoteEntry({ name: 'old.txt', path: '/old/old.txt' })]);
    await oldRequest;

    const state = useSftpStore.getState().getState('session-1');
    expect(state?.currentPath).toBe('/new');
    expect(state?.entries).toEqual([makeRemoteEntry({ name: 'new.txt', path: '/new/new.txt' })]);
    expect(state?.loading).toBe(false);
    expect(state?.error).toBeNull();
  });

  it('SFTP 旧目录请求失败不得结束最新请求的 loading 或写入错误', async () => {
    let rejectOld!: (error: unknown) => void;
    let resolveNew!: (entries: RemoteEntry[]) => void;
    mockInvoke
      .mockImplementationOnce(() => new Promise<RemoteEntry[]>((_, reject) => { rejectOld = reject; }))
      .mockImplementationOnce(() => new Promise<RemoteEntry[]>((resolve) => { resolveNew = resolve; }));

    const oldRequest = useSftpStore.getState().listDir('session-1', '/old');
    const newRequest = useSftpStore.getState().listDir('session-1', '/new');
    // 旧请求失败，最新请求仍挂起
    rejectOld({ code: 'SftpPermissionDenied', detail: '/old' });
    await oldRequest;

    const pending = useSftpStore.getState().getState('session-1');
    expect(pending?.loading).toBe(true);
    expect(pending?.error).toBeNull();

    resolveNew([makeRemoteEntry({ name: 'new.txt', path: '/new/new.txt' })]);
    await newRequest;
    const state = useSftpStore.getState().getState('session-1');
    expect(state?.loading).toBe(false);
    expect(state?.error).toBeNull();
    expect(state?.currentPath).toBe('/new');
  });

  it('SFTP 旧目录请求成功不得结束最新请求的 loading 或写入投影', async () => {
    let resolveOld!: (entries: RemoteEntry[]) => void;
    let resolveNew!: (entries: RemoteEntry[]) => void;
    mockInvoke
      .mockImplementationOnce(() => new Promise<RemoteEntry[]>((resolve) => { resolveOld = resolve; }))
      .mockImplementationOnce(() => new Promise<RemoteEntry[]>((resolve) => { resolveNew = resolve; }));

    const oldRequest = useSftpStore.getState().listDir('session-1', '/old');
    const newRequest = useSftpStore.getState().listDir('session-1', '/new');
    // 旧请求成功，最新请求仍挂起
    resolveOld([makeRemoteEntry({ name: 'old.txt', path: '/old/old.txt' })]);
    await oldRequest;

    const pending = useSftpStore.getState().getState('session-1');
    expect(pending?.loading).toBe(true);
    expect(pending?.entries).toHaveLength(0);
    expect(pending?.currentPath).toBe('/');

    resolveNew([makeRemoteEntry({ name: 'new.txt', path: '/new/new.txt' })]);
    await newRequest;
    const state = useSftpStore.getState().getState('session-1');
    expect(state?.loading).toBe(false);
    expect(state?.currentPath).toBe('/new');
    expect(state?.entries.map((entry) => entry.name)).toEqual(['new.txt']);
  });

  it('SFTP 最新目录请求失败后旧请求成功不得倒退错误与路径', async () => {
    let resolveOld!: (entries: RemoteEntry[]) => void;
    let rejectNew!: (error: unknown) => void;
    mockInvoke
      .mockImplementationOnce(() => new Promise<RemoteEntry[]>((resolve) => { resolveOld = resolve; }))
      .mockImplementationOnce(() => new Promise<RemoteEntry[]>((_, reject) => { rejectNew = reject; }));

    const oldRequest = useSftpStore.getState().listDir('session-1', '/old');
    const newRequest = useSftpStore.getState().listDir('session-1', '/new');
    // 最新请求失败：错误按既有契约展示，路径停留在最后成功加载的位置
    rejectNew({ code: 'SftpPathNotFound', detail: '/new' });
    await newRequest;
    // 旧请求成功后不得覆盖最新投影（错误、路径、条目均不得倒退）
    resolveOld([makeRemoteEntry({ name: 'old.txt', path: '/old/old.txt' })]);
    await oldRequest;

    const state = useSftpStore.getState().getState('session-1');
    expect(state?.currentPath).toBe('/');
    expect(state?.error).toEqual({ code: 'SftpPathNotFound', detail: '/new' });
    expect(state?.entries).toHaveLength(0);
    expect(state?.loading).toBe(false);
  });

  it('SFTP 不同会话的目录请求序号互不影响', async () => {
    let resolveAOld!: (entries: RemoteEntry[]) => void;
    let resolveANew!: (entries: RemoteEntry[]) => void;
    let resolveB!: (entries: RemoteEntry[]) => void;
    mockInvoke
      .mockImplementationOnce(() => new Promise<RemoteEntry[]>((resolve) => { resolveAOld = resolve; }))
      .mockImplementationOnce(() => new Promise<RemoteEntry[]>((resolve) => { resolveB = resolve; }))
      .mockImplementationOnce(() => new Promise<RemoteEntry[]>((resolve) => { resolveANew = resolve; }));

    useSftpStore.getState().ensureState('a');
    useSftpStore.getState().ensureState('b');

    const aOld = useSftpStore.getState().listDir('a', '/a-old');
    const b = useSftpStore.getState().listDir('b', '/b');
    const aNew = useSftpStore.getState().listDir('a', '/a-new');
    resolveB([makeRemoteEntry({ name: 'b.txt', path: '/b/b.txt' })]);
    await b;
    resolveANew([makeRemoteEntry({ name: 'a-new.txt', path: '/a-new/a-new.txt' })]);
    await aNew;
    resolveAOld([makeRemoteEntry({ name: 'a-old.txt', path: '/a-old/a-old.txt' })]);
    await aOld;

    expect(useSftpStore.getState().getState('a')?.currentPath).toBe('/a-new');
    expect(useSftpStore.getState().getState('a')?.entries.map((entry) => entry.name)).toEqual(['a-new.txt']);
    expect(useSftpStore.getState().getState('b')?.currentPath).toBe('/b');
    expect(useSftpStore.getState().getState('b')?.entries.map((entry) => entry.name)).toEqual(['b.txt']);
  });

  it('SFTP 进度、完成和失败事件更新对应任务', () => {
    const task = makeTransferTask();
    useSftpStore.setState({ sessionStates: new Map([['session-1', {
      currentPath: '/', entries: [], selectedPaths: new Set(), loading: false, error: null,
      tasks: new Map([[task.taskId, task]]), taskActionErrors: new Map(), dirRequestSeq: 0,
    }]]) });
    useSftpStore.getState().applyProgress({ taskId: task.taskId, sessionId: 'session-1', transferredBytes: 20, totalBytes: 100, speedBps: 5 });
    expect(useSftpStore.getState().getState('session-1')?.tasks.get(task.taskId)?.speedBps).toBe(5);
    useSftpStore.getState().applyTaskStatus({ taskId: task.taskId, sessionId: 'session-1', status: 'Done', error: null });
    expect(useSftpStore.getState().getState('session-1')?.tasks.get(task.taskId)?.transferredBytes).toBe(task.totalBytes);
  });

  it('SFTP 选择、取消与会话清理保持在公共动作边界', async () => {
    mockInvoke.mockResolvedValue(undefined);
    useSftpStore.getState().toggleSelect('session-1', '/a');
    expect(useSftpStore.getState().getState('session-1')?.selectedPaths.has('/a')).toBe(true);
    await useSftpStore.getState().cancelTask('task-1', 'session-1');
    expect(mockInvoke).toHaveBeenCalledWith('sftp_cancel_task', { taskId: 'task-1' });
    useSftpStore.getState().clearSession('session-1');
    expect(useSftpStore.getState().getState('session-1')).toBeUndefined();
  });

  it('SFTP 下载 invoke 失败时错误写入文件浏览器错误区且不抛出未处理异常', async () => {
    mockInvoke.mockRejectedValueOnce({ code: 'SftpPathNotFound', detail: '/var/log/missing' });
    await expect(
      useSftpStore.getState().download('session-1', '/var/log/missing', '/tmp/missing'),
    ).resolves.toBeUndefined();
    expect(useSftpStore.getState().getState('session-1')?.error).toEqual({
      code: 'SftpPathNotFound', detail: '/var/log/missing',
    });
    expect(useSftpStore.getState().getState('session-1')?.tasks.size).toBe(0);
  });

  it('SFTP 上传 invoke 失败时错误写入文件浏览器错误区', async () => {
    mockInvoke.mockRejectedValueOnce({ code: 'SftpTransferError', detail: '本地文件不存在' });
    await expect(
      useSftpStore.getState().upload('session-1', '/tmp/ghost', '/var/log'),
    ).resolves.toBeUndefined();
    expect(useSftpStore.getState().getState('session-1')?.error?.code).toBe('SftpTransferError');
  });

  it('SFTP 下载请求显式携带 Reject 冲突策略（默认）', async () => {
    mockInvoke.mockResolvedValueOnce(makeTransferTask());
    await useSftpStore.getState().download('session-1', '/var/log/syslog', '/tmp/syslog');
    expect(mockInvoke).toHaveBeenCalledWith('sftp_download', {
      sessionId: 'session-1', remotePath: '/var/log/syslog', localPath: '/tmp/syslog',
      conflictStrategy: 'Reject',
    });
  });

  it('SFTP 上传请求显式携带 Reject 冲突策略（默认）', async () => {
    mockInvoke.mockResolvedValueOnce(makeTransferTask({ transferType: 'Upload' }));
    await useSftpStore.getState().upload('session-1', '/tmp/syslog', '/var/log');
    expect(mockInvoke).toHaveBeenCalledWith('sftp_upload', {
      sessionId: 'session-1', localPath: '/tmp/syslog', remotePath: '/var/log',
      conflictStrategy: 'Reject',
    });
  });

  it('SFTP 上传冲突确认后以 Overwrite 策略重新发起，并清除原任务行 actionError', async () => {
    const task = makeTransferTask({ transferType: 'Upload', status: 'Failed' });
    useSftpStore.setState({ sessionStates: new Map([['session-1', {
      currentPath: '/', entries: [], selectedPaths: new Set(), loading: false, error: null,
      tasks: new Map([[task.taskId, task]]),
      taskActionErrors: new Map([[task.taskId, { code: 'SftpTargetExists', detail: '/var/log/syslog' }]]),
      dirRequestSeq: 0,
    }]]) });
    mockInvoke.mockResolvedValueOnce(makeTransferTask({ transferType: 'Upload', taskId: 'task-overwrite-2' }));
    await useSftpStore.getState().upload('session-1', task.localPath, uploadTargetDir(task), task.taskId, 'Overwrite');
    expect(mockInvoke).toHaveBeenCalledWith('sftp_upload', {
      sessionId: 'session-1', localPath: task.localPath, remotePath: uploadTargetDir(task),
      conflictStrategy: 'Overwrite',
    });
    const state = useSftpStore.getState().getState('session-1');
    expect(state?.taskActionErrors.has(task.taskId)).toBe(false);
    expect(state?.tasks.has('task-overwrite-2')).toBe(true);
  });

  it('上传任务 Done 且用户仍位于目标目录时自动刷新该目录', async () => {
    const task = makeTransferTask({ transferType: 'Upload', remotePath: '/var/log/syslog', fileName: 'syslog', status: 'Running' });
    useSftpStore.setState({ sessionStates: new Map([['session-1', {
      currentPath: '/var/log', entries: [], selectedPaths: new Set(), loading: false, error: null,
      tasks: new Map([[task.taskId, task]]), taskActionErrors: new Map(), dirRequestSeq: 0,
    }]]) });
    mockInvoke.mockResolvedValueOnce([makeRemoteEntry()]);
    useSftpStore.getState().applyTaskStatus({ taskId: task.taskId, sessionId: 'session-1', status: 'Done', error: null });
    expect(mockInvoke).toHaveBeenCalledWith('sftp_list_dir', { sessionId: 'session-1', path: '/var/log' });
    await Promise.resolve();
    expect(useSftpStore.getState().getState('session-1')?.tasks.get(task.taskId)?.status).toBe('Done');
  });

  it('上传任务 Done 但用户已离开目标目录时不刷新，不把用户拉回目标目录', () => {
    const task = makeTransferTask({ transferType: 'Upload', remotePath: '/var/log/syslog', fileName: 'syslog', status: 'Running' });
    useSftpStore.setState({ sessionStates: new Map([['session-1', {
      currentPath: '/home/user', entries: [], selectedPaths: new Set(), loading: false, error: null,
      tasks: new Map([[task.taskId, task]]), taskActionErrors: new Map(), dirRequestSeq: 0,
    }]]) });
    useSftpStore.getState().applyTaskStatus({ taskId: task.taskId, sessionId: 'session-1', status: 'Done', error: null });
    expect(mockInvoke).not.toHaveBeenCalled();
    expect(useSftpStore.getState().getState('session-1')?.tasks.get(task.taskId)?.status).toBe('Done');
  });

  it('下载任务 Done 不触发目录刷新', () => {
    const task = makeTransferTask({ status: 'Running' });
    useSftpStore.setState({ sessionStates: new Map([['session-1', {
      currentPath: '/var/log', entries: [], selectedPaths: new Set(), loading: false, error: null,
      tasks: new Map([[task.taskId, task]]), taskActionErrors: new Map(), dirRequestSeq: 0,
    }]]) });
    useSftpStore.getState().applyTaskStatus({ taskId: task.taskId, sessionId: 'session-1', status: 'Done', error: null });
    expect(mockInvoke).not.toHaveBeenCalled();
  });

  it('上传 Done 触发的刷新服从最新目录请求规则：晚到的刷新响应不得覆盖更新的导航', async () => {
    const task = makeTransferTask({ transferType: 'Upload', remotePath: '/var/log/syslog', fileName: 'syslog', status: 'Running' });
    useSftpStore.setState({ sessionStates: new Map([['session-1', {
      currentPath: '/var/log', entries: [], selectedPaths: new Set(), loading: false, error: null,
      tasks: new Map([[task.taskId, task]]), taskActionErrors: new Map(), dirRequestSeq: 0,
    }]]) });
    let resolveRefresh!: (entries: RemoteEntry[]) => void;
    mockInvoke.mockImplementationOnce(() => new Promise<RemoteEntry[]>((resolve) => { resolveRefresh = resolve; }));
    useSftpStore.getState().applyTaskStatus({ taskId: task.taskId, sessionId: 'session-1', status: 'Done', error: null });

    // 刷新在途时用户导航到新目录：最新请求序号更高
    mockInvoke.mockResolvedValueOnce([makeRemoteEntry({ name: 'new.txt', path: '/new/new.txt' })]);
    await useSftpStore.getState().listDir('session-1', '/new');
    // 刷新响应晚到：不得让投影倒退到旧目录
    resolveRefresh([makeRemoteEntry({ name: 'syslog', path: '/var/log/syslog' })]);
    await Promise.resolve();

    const state = useSftpStore.getState().getState('session-1');
    expect(state?.currentPath).toBe('/new');
    expect(state?.entries.map((entry) => entry.path)).toEqual(['/new/new.txt']);
  });

  it('uploadTargetDir 从完整远端目标路径提取目标目录', () => {
    expect(uploadTargetDir({ remotePath: '/var/log/syslog', fileName: 'syslog' })).toBe('/var/log');
    expect(uploadTargetDir({ remotePath: '/syslog', fileName: 'syslog' })).toBe('/');
    expect(uploadTargetDir({ remotePath: '/a/b/c.txt', fileName: 'c.txt' })).toBe('/a/b');
  });

  it('SFTP 冲突确认后以 Overwrite 策略重新发起，并清除原任务行 actionError', async () => {
    const task = makeTransferTask({ status: 'Failed' });
    useSftpStore.setState({ sessionStates: new Map([['session-1', {
      currentPath: '/', entries: [], selectedPaths: new Set(), loading: false, error: null,
      tasks: new Map([[task.taskId, task]]),
      taskActionErrors: new Map([[task.taskId, { code: 'SftpTargetExists', detail: '/tmp/syslog' }]]),
      dirRequestSeq: 0,
    }]]) });
    mockInvoke.mockResolvedValueOnce(makeTransferTask({ taskId: 'task-overwrite-2' }));
    await useSftpStore.getState().download('session-1', task.remotePath, task.localPath, task.taskId, 'Overwrite');
    expect(mockInvoke).toHaveBeenCalledWith('sftp_download', {
      sessionId: 'session-1', remotePath: task.remotePath, localPath: task.localPath,
      conflictStrategy: 'Overwrite',
    });
    const state = useSftpStore.getState().getState('session-1');
    expect(state?.taskActionErrors.has(task.taskId)).toBe(false);
    expect(state?.tasks.has('task-overwrite-2')).toBe(true);
  });

  it('SFTP 重试 invoke 失败时仅在原任务行记录 actionError，不写入文件浏览器错误区', async () => {
    const task = makeTransferTask({ status: 'Failed' });
    useSftpStore.setState({ sessionStates: new Map([['session-1', {
      currentPath: '/', entries: [], selectedPaths: new Set(), loading: false, error: null,
      tasks: new Map([[task.taskId, task]]), taskActionErrors: new Map(), dirRequestSeq: 0,
    }]]) });
    mockInvoke.mockRejectedValueOnce({ code: 'SftpPermissionDenied', detail: '/var/log' });
    await expect(
      useSftpStore.getState().download('session-1', task.remotePath, task.localPath, task.taskId),
    ).resolves.toBeUndefined();
    const state = useSftpStore.getState().getState('session-1');
    expect(state?.taskActionErrors.get(task.taskId)).toEqual({
      code: 'SftpPermissionDenied', detail: '/var/log',
    });
    expect(state?.error).toBeNull();
  });

  it('SFTP 重试 invoke 成功时清除原任务行的 actionError', async () => {
    const task = makeTransferTask({ status: 'Failed' });
    useSftpStore.setState({ sessionStates: new Map([['session-1', {
      currentPath: '/', entries: [], selectedPaths: new Set(), loading: false, error: null,
      tasks: new Map([[task.taskId, task]]),
      taskActionErrors: new Map([[task.taskId, { code: 'SftpPermissionDenied', detail: '/var/log' }]]),
      dirRequestSeq: 0,
    }]]) });
    mockInvoke.mockResolvedValueOnce(makeTransferTask({ taskId: 'task-retry-2' }));
    await useSftpStore.getState().download('session-1', task.remotePath, task.localPath, task.taskId);
    const state = useSftpStore.getState().getState('session-1');
    expect(state?.taskActionErrors.has(task.taskId)).toBe(false);
    expect(state?.tasks.has('task-retry-2')).toBe(true);
  });

  it('SFTP 取消 invoke 失败时在对应任务行记录 actionError', async () => {
    const task = makeTransferTask({ status: 'Running' });
    useSftpStore.setState({ sessionStates: new Map([['session-1', {
      currentPath: '/', entries: [], selectedPaths: new Set(), loading: false, error: null,
      tasks: new Map([[task.taskId, task]]), taskActionErrors: new Map(), dirRequestSeq: 0,
    }]]) });
    mockInvoke.mockRejectedValueOnce({ code: 'SftpTaskNotFound', detail: task.taskId });
    await expect(
      useSftpStore.getState().cancelTask(task.taskId, 'session-1'),
    ).resolves.toBeUndefined();
    expect(useSftpStore.getState().getState('session-1')?.taskActionErrors.get(task.taskId)).toEqual({
      code: 'SftpTaskNotFound', detail: task.taskId,
    });
  });

  it('SFTP 任务到达终态时清除对应任务行的 actionError', () => {
    const task = makeTransferTask({ status: 'Running' });
    useSftpStore.setState({ sessionStates: new Map([['session-1', {
      currentPath: '/', entries: [], selectedPaths: new Set(), loading: false, error: null,
      tasks: new Map([[task.taskId, task]]),
      taskActionErrors: new Map([[task.taskId, { code: 'SftpTaskNotFound', detail: task.taskId }]]),
      dirRequestSeq: 0,
    }]]) });
    useSftpStore.getState().applyTaskStatus({
      taskId: task.taskId, sessionId: 'session-1', status: 'Cancelled', error: null,
    });
    expect(useSftpStore.getState().getState('session-1')?.taskActionErrors.has(task.taskId)).toBe(false);
  });

  it('监控任务事件先于 invoke 返回时补投最新状态，终态不丢失', async () => {
    let resolveInvoke!: (task: TaskInfo) => void;
    mockInvoke.mockImplementationOnce(() => new Promise<TaskInfo>((resolve) => { resolveInvoke = resolve; }));
    const startPromise = useMonitorStore.getState().startMonitoring('session-1');

    // invoke 返回前到达的事件：Running 先到，Failed 后到，latest-wins
    useMonitorStore.getState().applyTaskStatus({ taskId: 'task-1', status: TaskStatus.Running });
    useMonitorStore.getState().applyTaskStatus({ taskId: 'task-1', status: TaskStatus.Failed, error: { code: 'MonitorError', detail: 'collect failed' } });
    resolveInvoke(makeTaskInfo());
    await startPromise;

    expect(useMonitorStore.getState().tasks.get('task-1')?.status).toBe(TaskStatus.Failed);
    expect(useMonitorStore.getState().pendingTaskEvents.has('task-1')).toBe(false);
  });

  it('SFTP 终态事件先于 invoke 返回时补投，进度强制为总大小', async () => {
    let resolveInvoke!: (task: TransferTask) => void;
    mockInvoke.mockImplementationOnce(() => new Promise<TransferTask>((resolve) => { resolveInvoke = resolve; }));
    const downloadPromise = useSftpStore.getState().download('session-1', '/var/log/syslog', '/tmp/syslog');

    useSftpStore.getState().applyTaskStatus({ taskId: 'task-sftp-1', sessionId: 'session-1', status: 'Done', error: null });
    resolveInvoke(makeTransferTask());
    await downloadPromise;

    const task = useSftpStore.getState().getState('session-1')?.tasks.get('task-sftp-1');
    expect(task?.status).toBe('Done');
    expect(task?.transferredBytes).toBe(makeTransferTask().totalBytes);
    expect(useSftpStore.getState().pendingTaskEvents.has('task-sftp-1')).toBe(false);
  });

  it('clearSession 清理同会话的缓存任务事件并拒绝迟到事件落回缓存', () => {
    useSftpStore.getState().ensureState('session-1');
    useSftpStore.getState().ensureState('session-2');
    useSftpStore.getState().applyTaskStatus({ taskId: 'task-x', sessionId: 'session-1', status: 'Running', error: null });
    useSftpStore.getState().applyTaskStatus({ taskId: 'task-y', sessionId: 'session-2', status: 'Running', error: null });
    useSftpStore.getState().clearSession('session-1');
    expect(useSftpStore.getState().pendingTaskEvents.has('task-x')).toBe(false);
    expect(useSftpStore.getState().pendingTaskEvents.has('task-y')).toBe(true);
    // 关闭后到达的迟到状态事件（后端 cleanup 的 Cancelled）直接丢弃，不再落回缓存
    useSftpStore.getState().applyTaskStatus({ taskId: 'task-x', sessionId: 'session-1', status: 'Cancelled', error: null });
    expect(useSftpStore.getState().pendingTaskEvents.has('task-x')).toBe(false);
  });

  it('关闭后迟到的目录失败不得重新创建 SFTP projection', async () => {
    let rejectRequest!: (error: unknown) => void;
    mockInvoke.mockImplementationOnce(() => new Promise<RemoteEntry[]>((_, reject) => { rejectRequest = reject; }));
    useSftpStore.getState().ensureState('session-1');

    const request = useSftpStore.getState().listDir('session-1', '/');
    useSftpStore.getState().clearSession('session-1');
    rejectRequest({ code: 'SessionNotFound', detail: 'session-1' });
    await request;

    expect(useSftpStore.getState().getState('session-1')).toBeUndefined();
  });

  it('关闭中的会话终态进入有界近期传输记录', () => {
    useSftpStore.getState().ensureState('session-1');
    useSftpStore.getState().markSessionClosing('session-1');
    useSftpStore.getState().applyFinishedTask(makeTransferTask({ status: 'Done' }));

    expect(useSftpStore.getState().recentTransfers).toHaveLength(1);
    expect(useSftpStore.getState().recentTransfers[0]?.status).toBe('Done');
  });

  it('打开会话时从后端权威快照恢复任务投影', async () => {
    mockInvoke.mockImplementation(async (command) => {
      if (command === 'open_session') return makeSession();
      if (command === 'start_monitoring') return makeTaskInfo();
      if (command === 'sftp_list_dir') return [];
      if (command === 'sftp_task_snapshot') return [makeTransferTask()];
      return undefined;
    });
    await useSessionStore.getState().openSession('host-1');
    expect(mockInvoke).toHaveBeenCalledWith('sftp_task_snapshot', { sessionId: 'session-1' });
    const task = useSftpStore.getState().getState('session-1')?.tasks.get('task-sftp-1');
    expect(task).toBeDefined();
  });

  it('SFTP 任务快照替换旧投影并补投加载期间早到的事件', async () => {
    let resolveInvoke!: (tasks: TransferTask[]) => void;
    mockInvoke.mockImplementationOnce(() => new Promise<TransferTask[]>((resolve) => { resolveInvoke = resolve; }));
    const stale = makeTransferTask({ taskId: 'task-stale', status: 'Done' });
    useSftpStore.setState({ sessionStates: new Map([['session-1', {
      currentPath: '/', entries: [], selectedPaths: new Set(), loading: false, error: null,
      tasks: new Map([[stale.taskId, stale]]), taskActionErrors: new Map(), dirRequestSeq: 0,
    }]]) });

    const loadPromise = useSftpStore.getState().loadTaskSnapshot('session-1');
    // invoke 返回前到达的早到事件：任务未知，先进入缓存
    useSftpStore.getState().applyTaskStatus({ taskId: 'task-sftp-1', sessionId: 'session-1', status: 'Done', error: null });
    resolveInvoke([makeTransferTask()]);
    await loadPromise;

    const state = useSftpStore.getState().getState('session-1');
    expect(state?.tasks.has('task-stale')).toBe(false);
    const task = state?.tasks.get('task-sftp-1');
    expect(task?.status).toBe('Done');
    expect(task?.transferredBytes).toBe(makeTransferTask().totalBytes);
    expect(useSftpStore.getState().pendingTaskEvents.has('task-sftp-1')).toBe(false);
  });

  it('SFTP 快照返回时保留请求开始后入队的新任务', async () => {
    let resolveSnapshot!: (tasks: TransferTask[]) => void;
    mockInvoke.mockImplementationOnce(() => new Promise<TransferTask[]>((resolve) => { resolveSnapshot = resolve; }));
    const fresh = makeTransferTask({ taskId: 'task-fresh', createdAt: Date.now() });
    mockInvoke.mockResolvedValueOnce(fresh); // sftp_download enqueue 响应
    const loadPromise = useSftpStore.getState().loadTaskSnapshot('session-1');
    await useSftpStore.getState().download('session-1', '/var/log/syslog', '/tmp/syslog');
    resolveSnapshot([]); // 后端快照采集早于 enqueue：不包含新任务
    await loadPromise;

    const state = useSftpStore.getState().getState('session-1');
    expect(state?.tasks.get('task-fresh')?.status).toBe('Pending');
    expect(state?.tasks.size).toBe(1);
  });

  it('SFTP 任务快照 invoke 失败时写入文件浏览器错误区', async () => {
    mockInvoke.mockRejectedValueOnce({ code: 'SftpChannelError', detail: 'session 已关闭' });
    await expect(useSftpStore.getState().loadTaskSnapshot('session-1')).resolves.toBeUndefined();
    expect(useSftpStore.getState().getState('session-1')?.error).toEqual({
      code: 'SftpChannelError', detail: 'session 已关闭',
    });
  });

  it('SFTP 清除终态任务仅移除终态、保留活动任务并清理对应 actionError', async () => {
    const done = makeTransferTask({ taskId: 'task-done', status: 'Done' });
    const failed = makeTransferTask({ taskId: 'task-failed', status: 'Failed' });
    const running = makeTransferTask({ taskId: 'task-running', status: 'Running' });
    useSftpStore.setState({ sessionStates: new Map([['session-1', {
      currentPath: '/', entries: [], selectedPaths: new Set(), loading: false, error: null,
      tasks: new Map([[done.taskId, done], [failed.taskId, failed], [running.taskId, running]]),
      taskActionErrors: new Map([[done.taskId, { code: 'SftpTaskNotFound', detail: done.taskId }]]),
      dirRequestSeq: 0,
    }]]) });
    mockInvoke.mockResolvedValue(undefined);
    await useSftpStore.getState().clearTerminalTasks('session-1');
    expect(mockInvoke).toHaveBeenCalledWith('sftp_clear_terminal_tasks', { sessionId: 'session-1' });
    const state = useSftpStore.getState().getState('session-1');
    expect(state?.tasks.has('task-running')).toBe(true);
    expect(state?.tasks.has('task-done')).toBe(false);
    expect(state?.tasks.has('task-failed')).toBe(false);
    expect(state?.taskActionErrors.has('task-done')).toBe(false);
  });

  it('SFTP 清除终态任务 invoke 失败时写入文件浏览器错误区', async () => {
    mockInvoke.mockRejectedValueOnce({ code: 'SftpChannelError', detail: 'registry locked' });
    await expect(useSftpStore.getState().clearTerminalTasks('session-1')).resolves.toBeUndefined();
    expect(useSftpStore.getState().getState('session-1')?.error?.code).toBe('SftpChannelError');
  });
});
