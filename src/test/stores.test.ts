import { beforeEach, describe, expect, it, vi } from 'vitest';
import { invoke } from '@tauri-apps/api/core';
import { emitMockEvent, resetMockEvents } from '@tauri-apps/api/event';
import { filterHosts, groupHosts, useHostStore } from '@/stores/host';
import { DEFAULT_SIDEBAR_WIDTH, MIN_MAIN_PANEL_WIDTH, MIN_SIDEBAR_WIDTH, readCollapsedGroups, readMonitorCollapsed, useLayoutStore } from '@/stores/layout';
import { useMonitorStore } from '@/stores/monitor';
import { useSessionStore } from '@/stores/session';
import { useSftpStore } from '@/stores/sftp';
import { ConnectionPhase, SessionStatus } from '@/types/session';
import { TaskStatus } from '@/types/monitor';
import { makeHost, makeRemoteEntry, makeSession, makeSnapshot, makeTaskInfo, makeTransferTask } from './fixtures';

const mockInvoke = vi.mocked(invoke);

/** 重置所有 Zustand store 和 Tauri 边界 mock。 */
function resetStores() {
  useHostStore.setState(useHostStore.getInitialState(), true);
  useLayoutStore.setState(useLayoutStore.getInitialState(), true);
  useMonitorStore.setState(useMonitorStore.getInitialState(), true);
  useSessionStore.setState(useSessionStore.getInitialState(), true);
  useSftpStore.setState(useSftpStore.getInitialState(), true);
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
    expect(useSessionStore.getState().activeView).toBe(session.sessionId);
    expect(useMonitorStore.getState().sessionTaskMap.get(session.sessionId)).toBe(task.taskId);
  });

  it('监控启动失败不阻断会话打开', async () => {
    mockInvoke.mockImplementation(async (command) => {
      if (command === 'open_session') return makeSession();
      throw new Error('monitor failed');
    });
    await expect(useSessionStore.getState().openSession('host-1')).resolves.toMatchObject({ sessionId: 'session-1' });
  });

  it('会话状态和进度事件更新公开状态', async () => {
    mockInvoke.mockImplementation(async (command) => command === 'open_session' ? makeSession() : makeTaskInfo());
    await useSessionStore.getState().openSession('host-1');
    const cleanup = await useSessionStore.getState().initListeners();
    emitMockEvent('session:progress', { sessionId: 'session-1', phase: ConnectionPhase.SshHandshake, message: '', timestamp: Date.now() });
    expect(useSessionStore.getState().statusMessage).toContain('SSH 握手');
    emitMockEvent('session:status', { sessionId: 'session-1', status: SessionStatus.AuthFailed, message: null });
    expect(useSessionStore.getState().statusMessage).toContain('认证失败');
    cleanup();
  });

  it('会话状态只消费后端事实并在连接成功时初始化文件传输', async () => {
    mockInvoke.mockImplementation(async (command) => {
      if (command === 'open_session') return makeSession();
      if (command === 'start_monitoring') return makeTaskInfo();
      if (command === 'sftp_list_dir') return [];
      return undefined;
    });
    await useSessionStore.getState().openSession('host-1');
    const cleanup = await useSessionStore.getState().initListeners();
    mockInvoke.mockClear();

    emitMockEvent('session:status', {
      sessionId: 'session-1', status: SessionStatus.Connected, message: null,
    });

    expect(mockInvoke).toHaveBeenCalledWith('sftp_list_dir', { sessionId: 'session-1', path: '/' });
    expect(mockInvoke).not.toHaveBeenCalledWith('sync_session_status', expect.anything());
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

  it('关闭活动会话只调用后端 teardown 并清理前端 projection', async () => {
    const task = makeTaskInfo();
    useSessionStore.setState({ sessions: new Map([['session-1', makeSession()]]), activeView: 'session-1' });
    useMonitorStore.setState({
      sessionTaskMap: new Map([['session-1', task.taskId]]),
      tasks: new Map([[task.taskId, task]]),
      snapshots: new Map([['session-1', makeSnapshot()]]),
    });
    mockInvoke.mockResolvedValue(undefined);
    await useSessionStore.getState().closeSession('session-1');

    expect(useSessionStore.getState().activeView).toBeNull();
    expect(mockInvoke).toHaveBeenCalledWith('close_session', { sessionId: 'session-1' });
    expect(mockInvoke).not.toHaveBeenCalledWith('stop_monitoring', expect.anything());
    expect(useMonitorStore.getState().sessionTaskMap.has('session-1')).toBe(false);
    expect(useMonitorStore.getState().tasks.has(task.taskId)).toBe(false);
    expect(useMonitorStore.getState().snapshots.has('session-1')).toBe(false);
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

  it('SFTP 目录成功与失败分别更新 entries 和 error', async () => {
    mockInvoke.mockResolvedValueOnce([makeRemoteEntry()]).mockRejectedValueOnce(new Error('denied'));
    await useSftpStore.getState().listDir('session-1', '/var/log');
    expect(useSftpStore.getState().getState('session-1')?.entries).toHaveLength(1);
    await useSftpStore.getState().listDir('session-1', '/root');
    expect(useSftpStore.getState().getState('session-1')?.error).toContain('denied');
  });

  it('SFTP 多会话状态互不影响', async () => {
    mockInvoke.mockResolvedValueOnce([makeRemoteEntry()]).mockResolvedValueOnce([]);
    await useSftpStore.getState().listDir('a', '/a');
    await useSftpStore.getState().listDir('b', '/b');
    expect(useSftpStore.getState().getState('a')?.entries).toHaveLength(1);
    expect(useSftpStore.getState().getState('b')?.entries).toHaveLength(0);
  });

  it('SFTP 进度、完成和失败事件更新对应任务', () => {
    const task = makeTransferTask();
    useSftpStore.setState({ sessionStates: new Map([['session-1', {
      currentPath: '/', entries: [], selectedPaths: new Set(), loading: false, error: null,
      tasks: new Map([[task.taskId, task]]),
    }]]) });
    useSftpStore.getState().applyProgress({ taskId: task.taskId, sessionId: 'session-1', transferredBytes: 20, totalBytes: 100, speedBps: 5 });
    expect(useSftpStore.getState().getState('session-1')?.tasks.get(task.taskId)?.speedBps).toBe(5);
    useSftpStore.getState().applyTaskStatus({ taskId: task.taskId, sessionId: 'session-1', status: 'Done', errorMessage: null });
    expect(useSftpStore.getState().getState('session-1')?.tasks.get(task.taskId)?.transferredBytes).toBe(task.totalBytes);
  });

  it('SFTP 选择、取消与会话清理保持在公共动作边界', async () => {
    mockInvoke.mockResolvedValue(undefined);
    useSftpStore.getState().toggleSelect('session-1', '/a');
    expect(useSftpStore.getState().getState('session-1')?.selectedPaths.has('/a')).toBe(true);
    await useSftpStore.getState().cancelTask('task-1');
    expect(mockInvoke).toHaveBeenCalledWith('sftp_cancel_task', { taskId: 'task-1' });
    useSftpStore.getState().clearSession('session-1');
    expect(useSftpStore.getState().getState('session-1')).toBeUndefined();
  });

  it('监控任务事件先于 invoke 返回时补投最新状态，终态不丢失', async () => {
    let resolveInvoke!: (task: TaskInfo) => void;
    mockInvoke.mockImplementationOnce(() => new Promise<TaskInfo>((resolve) => { resolveInvoke = resolve; }));
    const startPromise = useMonitorStore.getState().startMonitoring('session-1');

    // invoke 返回前到达的事件：Running 先到，Failed 后到，latest-wins
    useMonitorStore.getState().applyTaskStatus({ taskId: 'task-1', status: TaskStatus.Running });
    useMonitorStore.getState().applyTaskStatus({ taskId: 'task-1', status: TaskStatus.Failed, message: 'collect failed' });
    resolveInvoke(makeTaskInfo());
    await startPromise;

    expect(useMonitorStore.getState().tasks.get('task-1')?.status).toBe(TaskStatus.Failed);
    expect(useMonitorStore.getState().pendingTaskEvents.has('task-1')).toBe(false);
  });

  it('SFTP 终态事件先于 invoke 返回时补投，进度强制为总大小', async () => {
    let resolveInvoke!: (task: TransferTask) => void;
    mockInvoke.mockImplementationOnce(() => new Promise<TransferTask>((resolve) => { resolveInvoke = resolve; }));
    const downloadPromise = useSftpStore.getState().download('session-1', '/var/log/syslog', '/tmp/syslog');

    useSftpStore.getState().applyTaskStatus({ taskId: 'task-sftp-1', sessionId: 'session-1', status: 'Done', errorMessage: null });
    resolveInvoke(makeTransferTask());
    await downloadPromise;

    const task = useSftpStore.getState().getState('session-1')?.tasks.get('task-sftp-1');
    expect(task?.status).toBe('Done');
    expect(task?.transferredBytes).toBe(makeTransferTask().totalBytes);
    expect(useSftpStore.getState().pendingTaskEvents.has('task-sftp-1')).toBe(false);
  });

  it('clearSession 清理同会话的缓存任务事件', () => {
    useSftpStore.getState().applyTaskStatus({ taskId: 'task-x', sessionId: 'session-1', status: 'Running', errorMessage: null });
    useSftpStore.getState().applyTaskStatus({ taskId: 'task-y', sessionId: 'session-2', status: 'Running', errorMessage: null });
    useSftpStore.getState().clearSession('session-1');
    expect(useSftpStore.getState().pendingTaskEvents.has('task-x')).toBe(false);
    expect(useSftpStore.getState().pendingTaskEvents.has('task-y')).toBe(true);
  });
});
