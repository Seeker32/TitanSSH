import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { create } from 'zustand';
import type { TaskInfo } from '@/types/monitor';
import type { ProcessInfo, ProcessSnapshot } from '@/types/process';
import { longTaskProjection } from './long-task';

/** 进程摘要排序档位。 */
export type ProcessSortMode = 'cpu' | 'memory';

interface ProcessState {
  snapshots: Map<string, ProcessSnapshot>;
  sortMode: ProcessSortMode;
  getTopProcesses: (sessionId: string) => ProcessInfo[];
  setSortMode: (mode: ProcessSortMode) => void;
  applySnapshot: (snapshot: ProcessSnapshot) => void;
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
  sortMode: 'cpu',

  /** 获取指定会话当前排序档位下的 top-5 进程。 */
  getTopProcesses(sessionId) {
    return topProcesses(get().snapshots.get(sessionId), get().sortMode);
  },

  /** 切换进程摘要排序档位。 */
  setSortMode(mode) {
    set({ sortMode: mode });
  },

  /** 缓存指定会话的全量进程快照。 */
  applySnapshot(snapshot) {
    if (!longTaskProjection.isSessionActive(snapshot.sessionId)) return;
    set((state) => ({ snapshots: new Map(state.snapshots).set(snapshot.sessionId, snapshot) }));
  },

  /** 主动拉取并保存指定会话的进程快照。 */
  async fetchSnapshot(sessionId) {
    const snapshot = await invoke<ProcessSnapshot>('get_process_status', { sessionId });
    get().applySnapshot(snapshot);
    return snapshot;
  },

  /** 启动指定会话的进程采样任务；共享投影负责关联与事件竞态。 */
  startMonitoring(sessionId) {
    return longTaskProjection.start('process', sessionId, () => invoke<TaskInfo>('start_process_monitoring', { sessionId }));
  },

  /** 停止指定会话的进程采样任务并解除会话关联。 */
  async stopMonitoring(sessionId) {
    await longTaskProjection.stop('process', sessionId, (taskId) => invoke('stop_process_monitoring', { taskId }));
  },

  /** 清理已关闭会话的进程快照。 */
  clearSession(sessionId) {
    set((state) => {
      const snapshots = new Map(state.snapshots);
      snapshots.delete(sessionId);
      return { snapshots };
    });
  },

  /** 注册进程快照监听，任务状态由共享投影统一监听。 */
  async initListeners() {
    const unlistenSnapshot = await listen<ProcessSnapshot>('process:snapshot', (event) => {
      get().applySnapshot(event.payload);
    });
    return () => unlistenSnapshot();
  },
}));
