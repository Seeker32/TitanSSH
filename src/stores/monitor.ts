import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { create } from 'zustand';
import type { MonitorSnapshot, NetworkTrendSample, TaskInfo } from '@/types/monitor';
import { longTaskProjection } from './long-task';

/** 所选网卡趋势保留的真实时间窗口。 */
const TREND_WINDOW_MILLIS = 60_000;

interface MonitorState {
  snapshots: Map<string, MonitorSnapshot>;
  selectedInterfaces: Map<string, string>;
  networkTrends: Map<string, NetworkTrendSample[]>;
  applySnapshot: (snapshot: MonitorSnapshot) => void;
  selectNetworkInterface: (sessionId: string, interfaceName: string) => void;
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

  /** 写入指定会话快照，维护接口选择并采集最近一分钟趋势。 */
  applySnapshot(snapshot) {
    if (!longTaskProjection.isSessionActive(snapshot.sessionId)) return;
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

  /** 主动拉取并保存指定会话的监控快照。 */
  async fetchSnapshot(sessionId) {
    const snapshot = await invoke<MonitorSnapshot>('get_monitor_status', { sessionId });
    get().applySnapshot(snapshot);
    return snapshot;
  },

  /** 启动指定会话的监控长任务；共享投影负责关联与事件竞态。 */
  startMonitoring(sessionId) {
    return longTaskProjection.start('monitor', sessionId, () => invoke<TaskInfo>('start_monitoring', { sessionId }));
  },

  /** 停止指定会话的监控长任务并清理关联。 */
  async stopMonitoring(sessionId) {
    await longTaskProjection.stop('monitor', sessionId, (taskId) => invoke('stop_monitoring', { taskId }));
  },

  /** 清理已由后端 teardown 的会话监控快照，不发送停止命令。 */
  clearSession(sessionId) {
    set((state) => {
      const snapshots = new Map(state.snapshots);
      const selectedInterfaces = new Map(state.selectedInterfaces);
      const networkTrends = new Map(state.networkTrends);
      snapshots.delete(sessionId);
      selectedInterfaces.delete(sessionId);
      networkTrends.delete(sessionId);
      return { snapshots, selectedInterfaces, networkTrends };
    });
  },

  /** 注册监控快照监听，任务状态由共享投影统一监听。 */
  async initListeners() {
    const unlistenSnapshot = await listen<MonitorSnapshot>('monitor:snapshot', (event) => {
      get().applySnapshot(event.payload);
    });
    return () => unlistenSnapshot();
  },
}));
