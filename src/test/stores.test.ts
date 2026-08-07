import { beforeEach, describe, expect, it, vi } from 'vitest';
import { invoke } from '@tauri-apps/api/core';
import { emitMockEvent, resetMockEvents } from '@tauri-apps/api/event';
import { useHostStore } from '@/stores/host';
import { DEFAULT_SIDEBAR_WIDTH, MIN_MAIN_PANEL_WIDTH, MIN_SIDEBAR_WIDTH, useLayoutStore } from '@/stores/layout';
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
    expect(mockInvoke).toHaveBeenNthCalledWith(3, 'delete_host', { hostId: host.id });
    expect(useHostStore.getState().hosts).toEqual([]);
  });

  it('主机加载失败保留错误并结束 loading', async () => {
    mockInvoke.mockRejectedValueOnce(new Error('offline'));
    await useHostStore.getState().loadHosts();
    expect(useHostStore.getState()).toMatchObject({ loading: false, error: 'Error: offline' });
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

    expect(useSessionStore.getState().activeView).toBe('home');
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
