import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { create } from 'zustand';
import type { TaskInfo, TaskStatusEvent } from '@/types/monitor';
import type { ProcessInfo, ProcessSnapshot } from '@/types/process';

/** 进程摘要排序档位。 */
export type ProcessSortMode = 'cpu' | 'memory';

interface ProcessState {
  snapshots: Map<string, ProcessSnapshot>;
  tasks: Map<string, TaskInfo>;
  sessionTaskMap: Map<string, string>;
  /** invoke 返回前到达的任务状态事件缓存，任务元数据到达后补投。 */
  pendingTaskEvents: Map<string, TaskStatusEvent>;
  sortMode: ProcessSortMode;
  /** 已清理会话的 epoch，阻止迟到快照重新创建前端 projection。 */
  sessionEpochs: Map<string, number>;
  /** 已清理任务的 ID，阻止后端 teardown 迟到状态重新进入缓存。 */
  ignoredTaskIds: Set<string>;
  /** 已关闭会话集合，阻止迟到快照重新创建投影。 */
  closedSessions: Set<string>;
  /** 启动序号，关闭或重复启动会使旧的 invoke 结果失效。 */
  startTokens: Map<string, number>;
  getSessionTask: (sessionId: string) => TaskInfo | null;
  getTopProcesses: (sessionId: string) => ProcessInfo[];
  setSortMode: (mode: ProcessSortMode) => void;
  applySnapshot: (snapshot: ProcessSnapshot) => void;
  applyTaskStatus: (event: TaskStatusEvent) => void;
  fetchSnapshot: (sessionId: string) => Promise<ProcessSnapshot>;
  startMonitoring: (sessionId: string) => Promise<TaskInfo>;
  stopMonitoring: (sessionId: string) => Promise<void>;
  clearSession: (sessionId: string) => void;
  initListeners: () => Promise<() => void>;
}

/** 将空值排在有效数值后，并用 PID 保证摘要顺序稳定。 */
function compareProcesses(left: ProcessInfo, right: ProcessInfo, mode: ProcessSortMode): number {
  const leftValue = mode === 'cpu' ? left.cpuPercent : left.memoryBytes;
  const rightValue = mode === 'cpu' ? right.cpuPercent : right.memoryBytes;
  if (leftValue === null && rightValue !== null) return 1;
  if (leftValue !== null && rightValue === null) return -1;
  if (leftValue !== rightValue) return (rightValue ?? 0) - (leftValue ?? 0);
  return left.pid - right.pid;
}

/** 从同一份全量快照派生排序后的前五进程。 */
export function topProcesses(snapshot: ProcessSnapshot | undefined, mode: ProcessSortMode): ProcessInfo[] {
  return snapshot ? [...snapshot.processes].sort((left, right) => compareProcesses(left, right, mode)).slice(0, 5) : [];
}

export const useProcessStore = create<ProcessState>((set, get) => ({
  snapshots: new Map(),
  tasks: new Map(),
  sessionTaskMap: new Map(),
  pendingTaskEvents: new Map(),
  sortMode: 'cpu',
  sessionEpochs: new Map(),
  ignoredTaskIds: new Set(),
  closedSessions: new Set(),
  startTokens: new Map(),

  /** 获取指定会话关联的进程采样任务。 */
  getSessionTask(sessionId) {
    const taskId = get().sessionTaskMap.get(sessionId);
    return taskId ? get().tasks.get(taskId) ?? null : null;
  },

  /** 获取指定会话当前排序档位下的 top-5 进程。 */
  getTopProcesses(sessionId) {
    return topProcesses(get().snapshots.get(sessionId), get().sortMode);
  },

  /** 切换进程摘要排序档位。 */
  setSortMode(mode) {
    set({ sortMode: mode });
  },

  /** 缓存指定会话最新的全量进程快照。 */
  applySnapshot(snapshot) {
    if (get().closedSessions.has(snapshot.sessionId)) return;
    set((state) => ({ snapshots: new Map(state.snapshots).set(snapshot.sessionId, snapshot) }));
  },

  /** 应用任务状态事件；未知任务缓存最新事件，元数据到达后补投。 */
  applyTaskStatus(event) {
    if (get().ignoredTaskIds.has(event.taskId)) return;
    const existing = get().tasks.get(event.taskId);
    if (!existing) {
      set((state) => ({ pendingTaskEvents: new Map(state.pendingTaskEvents).set(event.taskId, event) }));
      return;
    }
    set((state) => ({
      tasks: new Map(state.tasks).set(event.taskId, { ...existing, status: event.status, error: event.error }),
    }));
  },

  /** 主动拉取并保存指定会话的进程快照。 */
  async fetchSnapshot(sessionId) {
    const snapshot = await invoke<ProcessSnapshot>('get_process_status', { sessionId });
    get().applySnapshot(snapshot);
    return snapshot;
  },

  /** 启动指定会话的进程采样任务，并处理 invoke/事件竞态。 */
  async startMonitoring(sessionId) {
    const epoch = get().sessionEpochs.get(sessionId) ?? 0;
    const token = (get().startTokens.get(sessionId) ?? 0) + 1;
    set((state) => {
      const closedSessions = new Set(state.closedSessions);
      closedSessions.delete(sessionId);
      return { closedSessions, startTokens: new Map(state.startTokens).set(sessionId, token) };
    });
    const task = await invoke<TaskInfo>('start_process_monitoring', { sessionId });
    set((state) => {
      const pendingTaskEvents = new Map(state.pendingTaskEvents);
      const buffered = pendingTaskEvents.get(task.taskId);
      pendingTaskEvents.delete(task.taskId);
      if ((state.sessionEpochs.get(sessionId) ?? 0) !== epoch || state.startTokens.get(sessionId) !== token || state.closedSessions.has(sessionId)) {
        return {
          pendingTaskEvents,
          ignoredTaskIds: new Set(state.ignoredTaskIds).add(task.taskId),
        };
      }
      return {
        tasks: new Map(state.tasks).set(task.taskId, buffered ? { ...task, status: buffered.status, error: buffered.error } : task),
        sessionTaskMap: new Map(state.sessionTaskMap).set(sessionId, task.taskId),
        pendingTaskEvents,
      };
    });
    return task;
  },

  /** 停止指定会话的进程采样任务并解除会话关联。 */
  async stopMonitoring(sessionId) {
    const taskId = get().sessionTaskMap.get(sessionId);
    if (!taskId) return;
    await invoke('stop_process_monitoring', { taskId });
    set((state) => {
      const sessionTaskMap = new Map(state.sessionTaskMap);
      sessionTaskMap.delete(sessionId);
      return { sessionTaskMap };
    });
  },

  /** 清理已关闭会话的进程快照、任务与缓存事件。 */
  clearSession(sessionId) {
    set((state) => {
      const taskId = state.sessionTaskMap.get(sessionId);
      const sessionTaskMap = new Map(state.sessionTaskMap);
      sessionTaskMap.delete(sessionId);
      const snapshots = new Map(state.snapshots);
      snapshots.delete(sessionId);
      const tasks = new Map(state.tasks);
      const removedTaskIds = new Set<string>();
      for (const [candidateTaskId, task] of tasks) {
        if (candidateTaskId === taskId || task.sessionId === sessionId) {
          tasks.delete(candidateTaskId);
          removedTaskIds.add(candidateTaskId);
        }
      }
      const pendingTaskEvents = new Map(state.pendingTaskEvents);
      removedTaskIds.forEach((candidateTaskId) => pendingTaskEvents.delete(candidateTaskId));
      const ignoredTaskIds = new Set(state.ignoredTaskIds);
      removedTaskIds.forEach((candidateTaskId) => ignoredTaskIds.add(candidateTaskId));
      const sessionEpochs = new Map(state.sessionEpochs);
      sessionEpochs.set(sessionId, (sessionEpochs.get(sessionId) ?? 0) + 1);
      const closedSessions = new Set(state.closedSessions);
      closedSessions.add(sessionId);
      const startTokens = new Map(state.startTokens);
      startTokens.set(sessionId, (startTokens.get(sessionId) ?? 0) + 1);
      return { snapshots, tasks, sessionTaskMap, pendingTaskEvents, ignoredTaskIds, sessionEpochs, closedSessions, startTokens };
    });
  },

  /** 注册进程快照与共享任务状态事件，返回统一清理函数。 */
  async initListeners() {
    const unlistenSnapshot = await listen<ProcessSnapshot>('process:snapshot', (event) => {
      get().applySnapshot(event.payload);
    });
    const unlistenTask = await listen<TaskStatusEvent>('task:status', (event) => {
      get().applyTaskStatus(event.payload);
    });
    return () => {
      unlistenSnapshot();
      unlistenTask();
    };
  },
}));
