import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { create } from 'zustand';
import type { MonitorSnapshot, TaskInfo, TaskStatusEvent } from '@/types/monitor';

interface MonitorState {
  snapshots: Map<string, MonitorSnapshot>;
  tasks: Map<string, TaskInfo>;
  sessionTaskMap: Map<string, string>;
  /** invoke 返回前到达的任务状态事件缓存，任务元数据到达后补投 */
  pendingTaskEvents: Map<string, TaskStatusEvent>;
  getSessionTask: (sessionId: string) => TaskInfo | null;
  applySnapshot: (snapshot: MonitorSnapshot) => void;
  applyTaskStatus: (event: TaskStatusEvent) => void;
  fetchSnapshot: (sessionId: string) => Promise<MonitorSnapshot>;
  startMonitoring: (sessionId: string) => Promise<TaskInfo>;
  stopMonitoring: (sessionId: string) => Promise<void>;
  clearSession: (sessionId: string) => void;
  initListeners: () => Promise<() => void>;
}

export const useMonitorStore = create<MonitorState>((set, get) => ({
  snapshots: new Map(),
  tasks: new Map(),
  sessionTaskMap: new Map(),
  pendingTaskEvents: new Map(),

  /** 获取指定会话关联的监控任务。 */
  getSessionTask(sessionId) {
    const taskId = get().sessionTaskMap.get(sessionId);
    return taskId ? get().tasks.get(taskId) ?? null : null;
  },

  /** 写入指定会话的最新监控快照。 */
  applySnapshot(snapshot) {
    set((state) => ({ snapshots: new Map(state.snapshots).set(snapshot.sessionId, snapshot) }));
  },

  /** 应用长任务状态事件；未知任务缓存最新事件，元数据到达后补投。 */
  applyTaskStatus(event) {
    const existing = get().tasks.get(event.taskId);
    if (!existing) {
      set((state) => ({
        pendingTaskEvents: new Map(state.pendingTaskEvents).set(event.taskId, event),
      }));
      return;
    }
    set((state) => ({
      tasks: new Map(state.tasks).set(event.taskId, { ...existing, status: event.status }),
    }));
  },

  /** 主动拉取并保存指定会话的监控快照。 */
  async fetchSnapshot(sessionId) {
    const snapshot = await invoke<MonitorSnapshot>('get_monitor_status', { sessionId });
    get().applySnapshot(snapshot);
    return snapshot;
  },

  /** 启动指定会话的监控长任务；invoke 返回前到达的事件补投到任务状态。 */
  async startMonitoring(sessionId) {
    const task = await invoke<TaskInfo>('start_monitoring', { sessionId });
    set((state) => {
      const pendingTaskEvents = new Map(state.pendingTaskEvents);
      const buffered = pendingTaskEvents.get(task.taskId);
      pendingTaskEvents.delete(task.taskId);
      return {
        tasks: new Map(state.tasks).set(task.taskId, buffered ? { ...task, status: buffered.status } : task),
        sessionTaskMap: new Map(state.sessionTaskMap).set(sessionId, task.taskId),
        pendingTaskEvents,
      };
    });
    return task;
  },

  /** 停止指定会话的监控长任务并清理关联。 */
  async stopMonitoring(sessionId) {
    const taskId = get().sessionTaskMap.get(sessionId);
    if (!taskId) return;
    await invoke('stop_monitoring', { taskId });
    set((state) => {
      const sessionTaskMap = new Map(state.sessionTaskMap);
      sessionTaskMap.delete(sessionId);
      return { sessionTaskMap };
    });
  },

  /** 清理已由后端 teardown 的会话监控 projection，不再发送停止命令。 */
  clearSession(sessionId) {
    set((state) => {
      const taskId = state.sessionTaskMap.get(sessionId);
      const sessionTaskMap = new Map(state.sessionTaskMap);
      const snapshots = new Map(state.snapshots);
      const tasks = new Map(state.tasks);
      sessionTaskMap.delete(sessionId);
      snapshots.delete(sessionId);
      if (taskId) tasks.delete(taskId);
      return { sessionTaskMap, snapshots, tasks };
    });
  },

  /** 注册监控快照与长任务事件，返回统一清理函数。 */
  async initListeners() {
    const unlistenSnapshot = await listen<MonitorSnapshot>('monitor:snapshot', (event) => {
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
