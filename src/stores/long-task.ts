import { listen } from '@tauri-apps/api/event';
import { create } from 'zustand';
import type { TaskInfo, TaskStatusEvent, SamplingTaskType } from '@/types/monitor';

const MAX_PENDING_EVENTS = 32;

interface InFlightStart {
  token: number;
  promise: Promise<TaskInfo>;
}

interface LongTaskState {
  activeSessionTokens: Map<string, number>;
  taskIdsByScope: Map<string, string>;
  tasksById: Map<string, TaskInfo>;
  pendingEvents: Map<string, TaskStatusEvent>;
  inFlightStarts: Map<string, InFlightStart>;
}

const useLongTaskStore = create<LongTaskState>(() => ({
  activeSessionTokens: new Map(),
  taskIdsByScope: new Map(),
  tasksById: new Map(),
  pendingEvents: new Map(),
  inFlightStarts: new Map(),
}));

let nextToken = 0;
let listenerCleanup: (() => void) | null = null;
let listenerPromise: Promise<() => void> | null = null;

function scopeKey(taskType: SamplingTaskType, sessionId: string): string {
  return `${taskType}:${sessionId}`;
}

function belongsToSession(scope: string, sessionId: string): boolean {
  return scope === scopeKey('monitor', sessionId) || scope === scopeKey('process', sessionId);
}

function isCurrentStart(taskType: SamplingTaskType, sessionId: string, token: number, promise: Promise<TaskInfo>): boolean {
  const state = useLongTaskStore.getState();
  const current = state.inFlightStarts.get(scopeKey(taskType, sessionId));
  return state.activeSessionTokens.get(sessionId) === token && current?.promise === promise;
}

function clearPendingScope(state: LongTaskState, taskType: SamplingTaskType, sessionId: string): Map<string, TaskStatusEvent> {
  const pendingEvents = new Map(state.pendingEvents);
  for (const [taskId, event] of pendingEvents) {
    if (event.taskType === taskType && event.sessionId === sessionId) pendingEvents.delete(taskId);
  }
  return pendingEvents;
}

function applyTaskStatus(event: TaskStatusEvent): void {
  const state = useLongTaskStore.getState();
  if ((event.taskType !== 'monitor' && event.taskType !== 'process')
    || typeof event.taskId !== 'string' || event.taskId.length === 0
    || typeof event.sessionId !== 'string' || event.sessionId.length === 0) return;
  if (state.activeSessionTokens.get(event.sessionId) === undefined) return;

  const task = state.tasksById.get(event.taskId);
  if (task) {
    if (task.taskType !== event.taskType || task.sessionId !== event.sessionId) return;
    const tasksById = new Map(state.tasksById).set(event.taskId, {
      ...task, status: event.status, error: event.error,
    });
    useLongTaskStore.setState({ tasksById });
    return;
  }

  const scope = scopeKey(event.taskType, event.sessionId);
  if (!state.inFlightStarts.has(scope)) return;
  const pendingEvents = new Map(state.pendingEvents).set(event.taskId, event);
  while (pendingEvents.size > MAX_PENDING_EVENTS) {
    const oldest = pendingEvents.keys().next().value;
    if (oldest === undefined) break;
    pendingEvents.delete(oldest);
  }
  useLongTaskStore.setState({ pendingEvents });
}

/** 长任务投影的唯一公开操作接口；内部状态不向调用者暴露。 */
export const longTaskProjection = {
  /** 建立一个 Runtime Session 的前端有效期。 */
  activateSession(sessionId: string): void {
    useLongTaskStore.setState((state) => ({
      activeSessionTokens: new Map(state.activeSessionTokens).set(sessionId, ++nextToken),
    }));
  },

  /** 使 Session 及其所有任务 scope 失效，不发送后端 stop。 */
  invalidateSession(sessionId: string): void {
    useLongTaskStore.setState((state) => {
      const activeSessionTokens = new Map(state.activeSessionTokens);
      activeSessionTokens.delete(sessionId);
      const taskIdsByScope = new Map(state.taskIdsByScope);
      const taskIds = new Set<string>();
      for (const [scope, taskId] of taskIdsByScope) {
        if (belongsToSession(scope, sessionId)) {
          taskIdsByScope.delete(scope);
          taskIds.add(taskId);
        }
      }
      const tasksById = new Map(state.tasksById);
      for (const [taskId, task] of tasksById) {
        if (task.sessionId === sessionId || taskIds.has(taskId)) tasksById.delete(taskId);
      }
      const pendingEvents = new Map(state.pendingEvents);
      for (const [taskId, event] of pendingEvents) {
        if (event.sessionId === sessionId || taskIds.has(taskId)) pendingEvents.delete(taskId);
      }
      const inFlightStarts = new Map(state.inFlightStarts);
      for (const scope of inFlightStarts.keys()) {
        if (belongsToSession(scope, sessionId)) inFlightStarts.delete(scope);
      }
      return { activeSessionTokens, taskIdsByScope, tasksById, pendingEvents, inFlightStarts };
    });
  },

  /** 判断 Runtime Session 是否仍可接受前端投影。 */
  isSessionActive(sessionId: string): boolean {
    return useLongTaskStore.getState().activeSessionTokens.has(sessionId);
  },

  /** 启动采样任务并处理事件竞态、contract 校验与迟到结果。 */
  start(taskType: SamplingTaskType, sessionId: string, request: () => Promise<TaskInfo>): Promise<TaskInfo> {
    const state = useLongTaskStore.getState();
    const scope = scopeKey(taskType, sessionId);
    const token = state.activeSessionTokens.get(sessionId);
    if (token === undefined) return Promise.reject({ code: 'SessionNotFound', detail: sessionId });
    const currentTaskId = state.taskIdsByScope.get(scope);
    if (currentTaskId) {
      const currentTask = state.tasksById.get(currentTaskId);
      if (currentTask) return Promise.resolve(currentTask);
      const taskIdsByScope = new Map(state.taskIdsByScope);
      taskIdsByScope.delete(scope);
      useLongTaskStore.setState({ taskIdsByScope });
    }
    const inFlight = state.inFlightStarts.get(scope);
    if (inFlight) return inFlight.promise;

    let promise!: Promise<TaskInfo>;
    promise = Promise.resolve().then(request).then((task) => {
      if (!task || typeof task.taskId !== 'string' || task.taskId.length === 0
        || task.taskType !== taskType || task.sessionId !== sessionId) {
        throw { code: 'InvalidTaskInfo', detail: `task scope mismatch for ${scope}` };
      }
      if (!isCurrentStart(taskType, sessionId, token, promise)) return task;
      const current = useLongTaskStore.getState();
      const buffered = current.pendingEvents.get(task.taskId);
      const taskIdsByScope = new Map(current.taskIdsByScope).set(scope, task.taskId);
      const tasksById = new Map(current.tasksById).set(task.taskId, buffered ? {
        ...task, status: buffered.status, error: buffered.error,
      } : task);
      useLongTaskStore.setState({
        taskIdsByScope,
        tasksById,
        pendingEvents: clearPendingScope(current, taskType, sessionId),
      });
      return task;
    }).catch((error) => {
      const current = useLongTaskStore.getState();
      if (isCurrentStart(taskType, sessionId, token, promise)) {
        useLongTaskStore.setState({
          pendingEvents: clearPendingScope(current, taskType, sessionId),
        });
      }
      throw error;
    }).finally(() => {
      const current = useLongTaskStore.getState();
      if (current.inFlightStarts.get(scope)?.promise === promise) {
        const inFlightStarts = new Map(current.inFlightStarts);
        inFlightStarts.delete(scope);
        useLongTaskStore.setState({ inFlightStarts });
      }
    });
    useLongTaskStore.setState((current) => ({
      inFlightStarts: new Map(current.inFlightStarts).set(scope, { token, promise }),
    }));
    return promise;
  },

  /** 查找某个采样 scope 的当前任务。 */
  getTask(taskType: SamplingTaskType, sessionId: string): TaskInfo | null {
    const state = useLongTaskStore.getState();
    const taskId = state.taskIdsByScope.get(scopeKey(taskType, sessionId));
    return taskId ? state.tasksById.get(taskId) ?? null : null;
  },

  /** 停止某个采样 scope；失败时保留任务关联以支持重试。 */
  stop(taskType: SamplingTaskType, sessionId: string, request: (taskId: string) => Promise<void>): Promise<void> {
    const scope = scopeKey(taskType, sessionId);
    const state = useLongTaskStore.getState();
    const taskId = state.taskIdsByScope.get(scope);
    if (!taskId || !state.activeSessionTokens.has(sessionId)) return Promise.resolve();
    const token = state.activeSessionTokens.get(sessionId)!;
    return request(taskId).then(() => {
      const current = useLongTaskStore.getState();
      if (current.activeSessionTokens.get(sessionId) !== token || current.taskIdsByScope.get(scope) !== taskId) return;
      const taskIdsByScope = new Map(current.taskIdsByScope);
      taskIdsByScope.delete(scope);
      const tasksById = new Map(current.tasksById);
      tasksById.delete(taskId);
      useLongTaskStore.setState({ taskIdsByScope, tasksById });
    });
  },

  /** 注册唯一 task:status listener，并返回幂等清理函数。 */
  initListener(): Promise<() => void> {
    if (listenerCleanup) return Promise.resolve(listenerCleanup);
    if (listenerPromise) return listenerPromise;
    listenerPromise = listen<TaskStatusEvent>('task:status', (event) => applyTaskStatus(event.payload)).then((unlisten) => {
      const cleanup = () => {
        if (listenerCleanup !== cleanup) return;
        listenerCleanup = null;
        listenerPromise = null;
        unlisten();
      };
      listenerCleanup = cleanup;
      return cleanup;
    });
    return listenerPromise;
  },
};

/** React 订阅某个采样 scope 的当前任务。 */
export function useLongTask(taskType: SamplingTaskType, sessionId: string | null): TaskInfo | null {
  return useLongTaskStore((state) => {
    if (sessionId === null) return null;
    const taskId = state.taskIdsByScope.get(scopeKey(taskType, sessionId));
    return taskId ? state.tasksById.get(taskId) ?? null : null;
  });
}
