/**
 * P0 监控主流程闭环测试
 *
 * 覆盖：
 * 1. 打开会话后自动启动监控，taskId 写入 sessionTaskMap
 * 2. task:status 事件驱动 tasks 集合状态流转
 * 3. monitor:snapshot 事件写入快照并可通过 activeSnapshot 读取
 * 4. 关闭会话时自动停止监控，sessionTaskMap 清理
 * 5. 多会话并发监控互不干扰
 * 6. 监控启动失败不阻断会话主流程
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import { invoke } from '@tauri-apps/api/core';
import { emitMockEvent, resetMockEvents } from '@tauri-apps/api/event';
import { useMonitorStore } from '@/stores/monitor';
import { useSessionStore } from '@/stores/session';
import { TaskStatus, type TaskInfo } from '@/types/monitor';
import { makeSession, makeSnapshot } from './fixtures';

/** 生成测试用 TaskInfo */
function makeTaskInfo(overrides: Partial<TaskInfo> = {}): TaskInfo {
  return {
    task_id: 'task-1',
    task_type: 'monitor',
    session_id: 'session-1',
    status: TaskStatus.Pending,
    created_at: 1_710_000_000_000,
    ...overrides,
  };
}

describe('P0 监控主流程闭环', () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    vi.mocked(invoke).mockReset();
    resetMockEvents();
  });

  // P0-1: 打开会话后自动启动监控
  it('打开会话后自动调用 start_monitoring 并写入 sessionTaskMap', async () => {
    vi.mocked(invoke)
      .mockResolvedValueOnce(makeSession())          // open_session
      .mockResolvedValueOnce(makeTaskInfo());         // start_monitoring

    const sessionStore = useSessionStore();
    const monitorStore = useMonitorStore();

    await sessionStore.openSession('host-1');

    expect(invoke).toHaveBeenCalledWith('start_monitoring', { sessionId: 'session-1' });
    expect(monitorStore.sessionTaskMap.get('session-1')).toBe('task-1');
    expect(monitorStore.tasks.get('task-1')?.status).toBe(TaskStatus.Pending);
  });

  // P0-1: 关闭会话时自动停止监控
  it('关闭会话时自动调用 stop_monitoring 并清理 sessionTaskMap', async () => {
    vi.mocked(invoke)
      .mockResolvedValueOnce(makeSession())          // open_session
      .mockResolvedValueOnce(makeTaskInfo())         // start_monitoring
      .mockResolvedValueOnce(undefined)              // stop_monitoring
      .mockResolvedValueOnce(undefined);             // close_session

    const sessionStore = useSessionStore();
    const monitorStore = useMonitorStore();

    await sessionStore.openSession('host-1');
    await sessionStore.closeSession('session-1');

    expect(invoke).toHaveBeenCalledWith('stop_monitoring', { taskId: 'task-1' });
    expect(monitorStore.sessionTaskMap.has('session-1')).toBe(false);
  });

  // P0-2: task:status 事件驱动 tasks 集合状态流转
  it('task:status 事件将任务状态从 Pending 更新为 Running', async () => {
    vi.mocked(invoke)
      .mockResolvedValueOnce(makeSession())
      .mockResolvedValueOnce(makeTaskInfo());

    const sessionStore = useSessionStore();
    const monitorStore = useMonitorStore();
    const dispose = await monitorStore.initListeners();

    await sessionStore.openSession('host-1');

    emitMockEvent('task:status', {
      task_id: 'task-1',
      status: TaskStatus.Running,
    });

    expect(monitorStore.tasks.get('task-1')?.status).toBe(TaskStatus.Running);
    dispose();
  });

  // P0-2: task:status 完整状态流转 Pending → Running → Done
  it('task:status 事件完整流转 Pending → Running → Done', async () => {
    vi.mocked(invoke)
      .mockResolvedValueOnce(makeSession())
      .mockResolvedValueOnce(makeTaskInfo());

    const sessionStore = useSessionStore();
    const monitorStore = useMonitorStore();
    const dispose = await monitorStore.initListeners();

    await sessionStore.openSession('host-1');

    emitMockEvent('task:status', { task_id: 'task-1', status: TaskStatus.Running });
    expect(monitorStore.tasks.get('task-1')?.status).toBe(TaskStatus.Running);

    emitMockEvent('task:status', { task_id: 'task-1', status: TaskStatus.Done });
    expect(monitorStore.tasks.get('task-1')?.status).toBe(TaskStatus.Done);

    dispose();
  });

  // P0-1: monitor:snapshot 事件写入快照，activeSnapshot 可读取
  it('monitor:snapshot 事件写入快照后 activeSnapshot 返回最新数据', async () => {
    vi.mocked(invoke)
      .mockResolvedValueOnce(makeSession())
      .mockResolvedValueOnce(makeTaskInfo());

    const sessionStore = useSessionStore();
    const monitorStore = useMonitorStore();
    const dispose = await monitorStore.initListeners();

    await sessionStore.openSession('host-1');

    emitMockEvent('monitor:snapshot', makeSnapshot({ session_id: 'session-1', cpu_usage: 63.5 }));

    expect(monitorStore.activeSnapshot?.cpu_usage).toBe(63.5);
    expect(monitorStore.snapshots.get('session-1')?.cpu_usage).toBe(63.5);

    dispose();
  });

  // P0-1: getSessionTask 返回正确的任务信息
  it('getSessionTask 返回与会话关联的 TaskInfo', async () => {
    vi.mocked(invoke)
      .mockResolvedValueOnce(makeSession())
      .mockResolvedValueOnce(makeTaskInfo());

    const sessionStore = useSessionStore();
    const monitorStore = useMonitorStore();

    await sessionStore.openSession('host-1');

    const task = monitorStore.getSessionTask('session-1');
    expect(task?.task_id).toBe('task-1');
    expect(task?.task_type).toBe('monitor');
  });

  // P0-1: 多会话并发监控互不干扰
  it('多会话并发监控各自维护独立的 taskId 映射', async () => {
    vi.mocked(invoke)
      .mockResolvedValueOnce(makeSession({ session_id: 'session-1', host_id: 'host-1' }))
      .mockResolvedValueOnce(makeTaskInfo({ task_id: 'task-1', session_id: 'session-1' }))
      .mockResolvedValueOnce(makeSession({ session_id: 'session-2', host_id: 'host-2' }))
      .mockResolvedValueOnce(makeTaskInfo({ task_id: 'task-2', session_id: 'session-2' }));

    const sessionStore = useSessionStore();
    const monitorStore = useMonitorStore();

    await sessionStore.openSession('host-1');
    await sessionStore.openSession('host-2');

    expect(monitorStore.sessionTaskMap.get('session-1')).toBe('task-1');
    expect(monitorStore.sessionTaskMap.get('session-2')).toBe('task-2');
    expect(monitorStore.tasks.size).toBe(2);
  });

  // P0-1: 监控启动失败不阻断会话主流程
  it('start_monitoring 失败时会话仍正常打开', async () => {
    vi.mocked(invoke)
      .mockResolvedValueOnce(makeSession())          // open_session 成功
      .mockRejectedValueOnce(new Error('监控服务不可用')); // start_monitoring 失败

    const sessionStore = useSessionStore();
    const monitorStore = useMonitorStore();

    // 不应抛出异常
    const session = await sessionStore.openSession('host-1');

    expect(session.session_id).toBe('session-1');
    expect(sessionStore.activeView).toBe('session-1');
    // 监控映射为空，但会话正常
    expect(monitorStore.sessionTaskMap.has('session-1')).toBe(false);
  });

  // P0-2: task:status 对未知 taskId 不报错
  it('task:status 事件对未注册的 taskId 静默忽略', async () => {
    const monitorStore = useMonitorStore();
    const dispose = await monitorStore.initListeners();

    // 不应抛出异常
    expect(() => {
      emitMockEvent('task:status', { task_id: 'unknown-task', status: TaskStatus.Running });
    }).not.toThrow();

    expect(monitorStore.tasks.size).toBe(0);
    dispose();
  });

  // P0-1: 完整闭环：打开会话 → 启动监控 → 收到快照 → 收到状态 → 关闭会话 → 停止监控
  it('完整监控闭环：open → start → snapshot → status → close → stop', async () => {
    vi.mocked(invoke)
      .mockResolvedValueOnce(makeSession())          // open_session
      .mockResolvedValueOnce(makeTaskInfo())         // start_monitoring
      .mockResolvedValueOnce(undefined)              // stop_monitoring
      .mockResolvedValueOnce(undefined);             // close_session

    const sessionStore = useSessionStore();
    const monitorStore = useMonitorStore();
    const dispose = await monitorStore.initListeners();

    // 1. 打开会话，自动启动监控
    await sessionStore.openSession('host-1');
    expect(monitorStore.sessionTaskMap.get('session-1')).toBe('task-1');

    // 2. 收到任务状态变更
    emitMockEvent('task:status', { task_id: 'task-1', status: TaskStatus.Running });
    expect(monitorStore.tasks.get('task-1')?.status).toBe(TaskStatus.Running);

    // 3. 收到监控快照
    emitMockEvent('monitor:snapshot', makeSnapshot({ session_id: 'session-1', cpu_usage: 45.0 }));
    expect(monitorStore.activeSnapshot?.cpu_usage).toBe(45.0);

    // 4. 关闭会话，自动停止监控
    await sessionStore.closeSession('session-1');
    expect(invoke).toHaveBeenCalledWith('stop_monitoring', { taskId: 'task-1' });
    expect(monitorStore.sessionTaskMap.has('session-1')).toBe(false);

    dispose();
  });
});
