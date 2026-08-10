import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { create } from 'zustand';
import type { MonitorSnapshot, NetworkTrendSample, TaskInfo, TaskStatusEvent } from '@/types/monitor';

/** 所选网卡趋势保留的真实时间窗口。 */
const TREND_WINDOW_MILLIS = 60_000;

interface MonitorState {
  snapshots: Map<string, MonitorSnapshot>;
  selectedInterfaces: Map<string, string>;
  networkTrends: Map<string, NetworkTrendSample[]>;
  tasks: Map<string, TaskInfo>;
  sessionTaskMap: Map<string, string>;
  /** invoke 返回前到达的任务状态事件缓存，任务元数据到达后补投 */
  pendingTaskEvents: Map<string, TaskStatusEvent>;
  getSessionTask: (sessionId: string) => TaskInfo | null;
  applySnapshot: (snapshot: MonitorSnapshot) => void;
  selectNetworkInterface: (sessionId: string, interfaceName: string) => void;
  applyTaskStatus: (event: TaskStatusEvent) => void;
  fetchSnapshot: (sessionId: string) => Promise<MonitorSnapshot>;
  startMonitoring: (sessionId: string) => Promise<TaskInfo>;
  stopMonitoring: (sessionId: string) => Promise<void>;
  clearSession: (sessionId: string) => void;
  initListeners: () => Promise<() => void>;
}

export const useMonitorStore = create<MonitorState>((set, get) => ({
  snapshots: new Map(),
  selectedInterfaces: new Map(),
  networkTrends: new Map(),
  tasks: new Map(),
  sessionTaskMap: new Map(),
  pendingTaskEvents: new Map(),

  /** 获取指定会话关联的监控任务。 */
  getSessionTask(sessionId) {
    const taskId = get().sessionTaskMap.get(sessionId);
    return taskId ? get().tasks.get(taskId) ?? null : null;
  },

  /** 写入指定会话快照，维护接口选择并采集最近一分钟趋势。 */
  applySnapshot(snapshot) {
    set((state) => {
      const selectedInterfaces = new Map(state.selectedInterfaces);
      const networkTrends = new Map(state.networkTrends);
      let selected = selectedInterfaces.get(snapshot.sessionId);
      if (snapshot.network.available) {
        const interfaces = snapshot.network.interfaces;
        if (interfaces.length === 0) {
          selectedInterfaces.delete(snapshot.sessionId);
          networkTrends.delete(snapshot.sessionId);
          selected = undefined;
        } else {
          const nextSelected = interfaces.some((item) => item.name === selected) ? selected! : interfaces[0].name;
          if (nextSelected !== selected) {
            selectedInterfaces.set(snapshot.sessionId, nextSelected);
            networkTrends.delete(snapshot.sessionId);
          }
          selected = nextSelected;
        }
      }
      if (selected) {
        const current = snapshot.network.available
          ? snapshot.network.interfaces.find((item) => item.name === selected)
          : undefined;
        const sample: NetworkTrendSample = {
          timestamp: snapshot.timestamp,
          receiveBytesPerSecond: current?.receiveBytesPerSecond ?? null,
          transmitBytesPerSecond: current?.transmitBytesPerSecond ?? null,
        };
        networkTrends.set(snapshot.sessionId, [...(networkTrends.get(snapshot.sessionId) ?? []), sample]
          .filter((item) => item.timestamp >= snapshot.timestamp - TREND_WINDOW_MILLIS));
      }
      return {
        snapshots: new Map(state.snapshots).set(snapshot.sessionId, snapshot),
        selectedInterfaces,
        networkTrends,
      };
    });
  },

  /** 切换指定会话的当前网卡；仅接受最新可用快照中的候选接口。 */
  selectNetworkInterface(sessionId, interfaceName) {
    const snapshot = get().snapshots.get(sessionId);
    if (!snapshot?.network.available || !snapshot.network.interfaces.some((item) => item.name === interfaceName)) return;
    if (get().selectedInterfaces.get(sessionId) === interfaceName) return;
    set((state) => {
      const networkTrends = new Map(state.networkTrends);
      networkTrends.delete(sessionId);
      return { selectedInterfaces: new Map(state.selectedInterfaces).set(sessionId, interfaceName), networkTrends };
    });
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
      tasks: new Map(state.tasks).set(event.taskId, { ...existing, status: event.status, error: event.error }),
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
        tasks: new Map(state.tasks).set(task.taskId, buffered ? { ...task, status: buffered.status, error: buffered.error } : task),
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
      const selectedInterfaces = new Map(state.selectedInterfaces);
      const networkTrends = new Map(state.networkTrends);
      sessionTaskMap.delete(sessionId);
      snapshots.delete(sessionId);
      selectedInterfaces.delete(sessionId);
      networkTrends.delete(sessionId);
      if (taskId) tasks.delete(taskId);
      return { sessionTaskMap, snapshots, tasks, selectedInterfaces, networkTrends };
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
