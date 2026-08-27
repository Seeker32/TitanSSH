import { beforeEach, describe, expect, it } from 'vitest';
import { emitMockEvent, resetMockEvents } from '@tauri-apps/api/event';
import { longTaskProjection } from '@/stores/long-task';
import { TaskStatus, type TaskInfo, type TaskStatusEvent, type SamplingTaskType } from '@/types/monitor';

function task(taskType: SamplingTaskType, taskId: string, sessionId: string, status = TaskStatus.Pending): TaskInfo {
  return { taskId, taskType, sessionId, status, createdAt: 1 };
}

function status(taskId: string, taskType: SamplingTaskType, sessionId: string, nextStatus: TaskStatus): TaskStatusEvent {
  return { taskId, taskType, sessionId, status: nextStatus, error: null };
}

describe('long task projection', () => {
  let cleanup: (() => void) | undefined;

  beforeEach(() => {
    cleanup?.();
    cleanup = undefined;
    resetMockEvents();
    longTaskProjection.invalidateSession('session-1');
    longTaskProjection.invalidateSession('session-2');
  });

  it('补投 invoke 返回前到达的最新状态', async () => {
    cleanup = await longTaskProjection.initListener();
    longTaskProjection.activateSession('session-1');
    let resolveRequest!: (value: TaskInfo) => void;
    const started = longTaskProjection.start('monitor', 'session-1', () => new Promise((resolve) => { resolveRequest = resolve; }));
    await Promise.resolve();

    emitMockEvent('task:status', status('monitor-1', 'monitor', 'session-1', TaskStatus.Running));
    emitMockEvent('task:status', { ...status('monitor-1', 'monitor', 'session-1', TaskStatus.Failed), error: { code: 'MonitorError', detail: 'failed' } });
    resolveRequest(task('monitor', 'monitor-1', 'session-1'));
    await started;

    expect(longTaskProjection.getTask('monitor', 'session-1')).toMatchObject({
      taskId: 'monitor-1', status: TaskStatus.Failed, error: { code: 'MonitorError' },
    });
  });

  it('按 taskType 和 sessionId 隔离事件', async () => {
    cleanup = await longTaskProjection.initListener();
    longTaskProjection.activateSession('session-1');
    let resolveRequest!: (value: TaskInfo) => void;
    const started = longTaskProjection.start('monitor', 'session-1', () => new Promise((resolve) => { resolveRequest = resolve; }));
    await Promise.resolve();
    emitMockEvent('task:status', status('wrong-1', 'process', 'session-1', TaskStatus.Failed));
    emitMockEvent('task:status', status('monitor-1', 'monitor', 'session-1', TaskStatus.Running));
    resolveRequest(task('monitor', 'monitor-1', 'session-1'));
    await started;

    expect(longTaskProjection.getTask('monitor', 'session-1')?.status).toBe(TaskStatus.Running);
  });

  it('无效 Session 或非 start scope 的未知事件直接丢弃', async () => {
    cleanup = await longTaskProjection.initListener();
    emitMockEvent('task:status', status('monitor-1', 'monitor', 'session-1', TaskStatus.Failed));
    longTaskProjection.activateSession('session-1');
    const started = longTaskProjection.start('monitor', 'session-1', async () => task('monitor', 'monitor-1', 'session-1'));
    await started;

    expect(longTaskProjection.getTask('monitor', 'session-1')?.status).toBe(TaskStatus.Pending);
  });

  it('启动失败不留下任务，且并发 start 复用同一个 Promise', async () => {
    longTaskProjection.activateSession('session-1');
    let rejectRequest!: (reason?: unknown) => void;
    const failed = longTaskProjection.start('monitor', 'session-1', () => new Promise<TaskInfo>((_, reject) => { rejectRequest = reject; }));
    expect(longTaskProjection.start('monitor', 'session-1', () => { throw new Error('must not run'); })).toBe(failed);
    await Promise.resolve();
    rejectRequest(new Error('unavailable'));
    await expect(failed).rejects.toThrow('unavailable');
    expect(longTaskProjection.getTask('monitor', 'session-1')).toBeNull();
  });

  it('Session 关闭发生在 invoke 返回前时不重建 projection', async () => {
    cleanup = await longTaskProjection.initListener();
    longTaskProjection.activateSession('session-1');
    let resolveRequest!: (value: TaskInfo) => void;
    const started = longTaskProjection.start('process', 'session-1', () => new Promise((resolve) => { resolveRequest = resolve; }));
    await Promise.resolve();
    longTaskProjection.invalidateSession('session-1');
    resolveRequest(task('process', 'process-1', 'session-1'));
    await started;
    emitMockEvent('task:status', status('process-1', 'process', 'session-1', TaskStatus.Failed));

    expect(longTaskProjection.isSessionActive('session-1')).toBe(false);
    expect(longTaskProjection.getTask('process', 'session-1')).toBeNull();
  });

  it('校验 start 返回的 task scope', async () => {
    longTaskProjection.activateSession('session-1');

    await expect(longTaskProjection.start('monitor', 'session-1', async () => task('process', 'task-1', 'session-1')))
      .rejects.toMatchObject({ code: 'InvalidTaskInfo' });
    expect(longTaskProjection.getTask('monitor', 'session-1')).toBeNull();
  });

  it('stop 成功解除关联，失败保留关联', async () => {
    longTaskProjection.activateSession('session-1');
    await longTaskProjection.start('monitor', 'session-1', async () => task('monitor', 'monitor-1', 'session-1'));
    await expect(longTaskProjection.stop('monitor', 'session-1', async () => { throw new Error('busy'); })).rejects.toThrow('busy');
    expect(longTaskProjection.getTask('monitor', 'session-1')).not.toBeNull();
    await longTaskProjection.stop('monitor', 'session-1', async (taskId) => expect(taskId).toBe('monitor-1'));
    expect(longTaskProjection.getTask('monitor', 'session-1')).toBeNull();
  });

  it('旧任务在新 start 期间的迟到事件不能覆盖新任务', async () => {
    cleanup = await longTaskProjection.initListener();
    longTaskProjection.activateSession('session-1');
    await longTaskProjection.start('monitor', 'session-1', async () => task('monitor', 'old-task', 'session-1'));
    await longTaskProjection.stop('monitor', 'session-1', async () => {});
    let resolveRequest!: (value: TaskInfo) => void;
    const started = longTaskProjection.start('monitor', 'session-1', () => new Promise((resolve) => { resolveRequest = resolve; }));
    await Promise.resolve();
    emitMockEvent('task:status', status('old-task', 'monitor', 'session-1', TaskStatus.Failed));
    resolveRequest(task('monitor', 'new-task', 'session-1'));
    await started;

    expect(longTaskProjection.getTask('monitor', 'session-1')).toMatchObject({ taskId: 'new-task', status: TaskStatus.Pending });
  });

  it('listener cleanup 后停止接收事件', async () => {
    cleanup = await longTaskProjection.initListener();
    longTaskProjection.activateSession('session-1');
    await longTaskProjection.start('monitor', 'session-1', async () => task('monitor', 'monitor-1', 'session-1'));
    cleanup();
    emitMockEvent('task:status', status('monitor-1', 'monitor', 'session-1', TaskStatus.Failed));

    expect(longTaskProjection.getTask('monitor', 'session-1')?.status).toBe(TaskStatus.Pending);
  });
});
