import { beforeEach, describe, expect, it, vi } from 'vitest';
import { invoke } from '@tauri-apps/api/core';
import { emitMockEvent, resetMockEvents } from '@tauri-apps/api/event';
import { useProcessStore } from '@/stores/process';
import { TaskStatus } from '@/types/monitor';
import type { ProcessSnapshot } from '@/types/process';

const mockInvoke = vi.mocked(invoke);

function snapshot(overrides: Partial<ProcessSnapshot> = {}): ProcessSnapshot {
  return {
    sessionId: 'session-1',
    timestamp: 1_710_000_000_000,
    totalCount: 6,
    processes: [
      { pid: 1, ppid: 0, user: 'root', command: 'init', commandLine: '/sbin/init', cpuPercent: 1, memoryBytes: 10, state: 'S' },
      { pid: 2, ppid: 1, user: 'root', command: 'worker', commandLine: 'worker', cpuPercent: 90, memoryBytes: 20, state: 'R' },
      { pid: 3, ppid: 1, user: 'root', command: 'cache', commandLine: 'cache', cpuPercent: 20, memoryBytes: 90, state: 'S' },
      { pid: 4, ppid: 1, user: 'root', command: 'shell', commandLine: 'shell', cpuPercent: 50, memoryBytes: 30, state: 'S' },
      { pid: 5, ppid: 1, user: 'root', command: 'db', commandLine: 'db', cpuPercent: null, memoryBytes: null, state: 'S' },
      { pid: 6, ppid: 1, user: 'root', command: 'api', commandLine: 'api', cpuPercent: 40, memoryBytes: 50, state: 'S' },
    ],
    ...overrides,
  };
}

describe('process store', () => {
  beforeEach(() => {
    useProcessStore.setState(useProcessStore.getInitialState(), true);
    resetMockEvents();
    mockInvoke.mockReset();
  });

  it('缓存每个会话的最新快照并按 CPU 或内存派生 top-5', () => {
    useProcessStore.getState().applySnapshot(snapshot());

    expect(useProcessStore.getState().snapshots.get('session-1')?.timestamp).toBe(1_710_000_000_000);
    expect(useProcessStore.getState().getTopProcesses('session-1').map((item) => item.pid)).toEqual([2, 4, 6, 3, 1]);

    useProcessStore.getState().setSortMode('memory');
    expect(useProcessStore.getState().getTopProcesses('session-1').map((item) => item.pid)).toEqual([3, 6, 4, 2, 1]);
  });

  it('监听进程快照和任务状态事件，并缓冲 invoke 返回前的最新状态', async () => {
    const cleanup = await useProcessStore.getState().initListeners();
    emitMockEvent('process:snapshot', snapshot({ timestamp: 1_710_000_001_000 }));

    let resolveInvoke!: (task: unknown) => void;
    mockInvoke.mockImplementationOnce(() => new Promise((resolve) => { resolveInvoke = resolve; }));
    const start = useProcessStore.getState().startMonitoring('session-1');
    emitMockEvent('task:status', { taskId: 'process-task-1', status: TaskStatus.Running });
    emitMockEvent('task:status', { taskId: 'process-task-1', status: TaskStatus.Failed, error: { code: 'MonitorError', detail: 'failed' } });
    resolveInvoke({ taskId: 'process-task-1', taskType: 'process', sessionId: 'session-1', status: TaskStatus.Pending, createdAt: 1 });
    await start;

    expect(useProcessStore.getState().snapshots.get('session-1')?.timestamp).toBe(1_710_000_001_000);
    expect(useProcessStore.getState().tasks.get('process-task-1')?.status).toBe(TaskStatus.Failed);
    expect(useProcessStore.getState().pendingTaskEvents.has('process-task-1')).toBe(false);
    cleanup();
  });

  it('启动失败不留下任务投影，关闭会话清理快照和迟到任务事件', async () => {
    mockInvoke.mockRejectedValueOnce(new Error('process unavailable'));
    await expect(useProcessStore.getState().startMonitoring('session-1')).rejects.toThrow('process unavailable');

    const task = { taskId: 'process-task-1', taskType: 'process', sessionId: 'session-1', status: TaskStatus.Running, createdAt: 1 };
    useProcessStore.setState({
      snapshots: new Map([['session-1', snapshot()]]),
      tasks: new Map([[task.taskId, task]]),
      sessionTaskMap: new Map([['session-1', task.taskId]]),
    });
    useProcessStore.getState().clearSession('session-1');
    useProcessStore.getState().applyTaskStatus({ taskId: task.taskId, status: TaskStatus.Done });

    expect(useProcessStore.getState().snapshots.has('session-1')).toBe(false);
    expect(useProcessStore.getState().tasks.has(task.taskId)).toBe(false);
    expect(useProcessStore.getState().pendingTaskEvents.has(task.taskId)).toBe(false);
  });

  it('关闭中的启动结果和迟到快照都不会重新创建会话投影', async () => {
    let resolveInvoke!: (task: unknown) => void;
    mockInvoke.mockImplementationOnce(() => new Promise((resolve) => { resolveInvoke = resolve; }));
    const start = useProcessStore.getState().startMonitoring('session-1');
    useProcessStore.getState().clearSession('session-1');
    resolveInvoke({ taskId: 'process-task-1', taskType: 'process', sessionId: 'session-1', status: TaskStatus.Pending, createdAt: 1 });
    await start;
    useProcessStore.getState().applySnapshot(snapshot({ timestamp: 1_710_000_002_000 }));

    expect(useProcessStore.getState().tasks.has('process-task-1')).toBe(false);
    expect(useProcessStore.getState().snapshots.has('session-1')).toBe(false);
  });
});
