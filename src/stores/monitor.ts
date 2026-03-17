import { defineStore } from 'pinia';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { ref, computed } from 'vue';
import type { MonitorSnapshot, TaskInfo, TaskStatusEvent } from '@/types/monitor';
import { useSessionStore } from './session';

export const useMonitorStore = defineStore('monitor', () => {
  /** 监控快照集合，按 session_id 索引 */
  const snapshots = ref(new Map<string, MonitorSnapshot>());

  /** 长任务信息集合，按 task_id 索引 */
  const tasks = ref(new Map<string, TaskInfo>());

  /** 会话与监控任务的关联映射，按 session_id 索引 task_id */
  const sessionTaskMap = ref(new Map<string, string>());

  /** 当前活跃会话的监控快照 */
  const activeSnapshot = computed(() => {
    const sessionStore = useSessionStore();
    if (sessionStore.activeView !== 'home') {
      return snapshots.value.get(sessionStore.activeView) ?? null;
    }
    return null;
  });

  /** 获取指定会话关联的任务信息，若不存在则返回 null */
  function getSessionTask(sessionId: string): TaskInfo | null {
    const taskId = sessionTaskMap.value.get(sessionId);
    if (!taskId) return null;
    return tasks.value.get(taskId) ?? null;
  }

  /** 更新指定会话的监控快照 */
  function applySnapshot(snapshot: MonitorSnapshot) {
    snapshots.value = new Map(snapshots.value).set(snapshot.session_id, snapshot);
  }

  /** 处理 task:status 事件，将任务状态写入 tasks 集合 */
  function applyTaskStatus(event: TaskStatusEvent) {
    const existing = tasks.value.get(event.task_id);
    if (existing) {
      tasks.value = new Map(tasks.value).set(event.task_id, {
        ...existing,
        status: event.status,
      });
    }
  }

  /** 主动拉取指定会话的监控快照 */
  async function fetchSnapshot(sessionId: string) {
    const snapshot = await invoke<MonitorSnapshot>('get_monitor_status', { sessionId });
    applySnapshot(snapshot);
    return snapshot;
  }

  /**
   * 启动指定会话的监控任务，返回 TaskInfo
   * 同时将 taskId 与 sessionId 的关联写入 sessionTaskMap 和 tasks
   */
  async function startMonitoring(sessionId: string) {
    const taskInfo = await invoke<TaskInfo>('start_monitoring', { sessionId });
    tasks.value = new Map(tasks.value).set(taskInfo.task_id, taskInfo);
    sessionTaskMap.value = new Map(sessionTaskMap.value).set(sessionId, taskInfo.task_id);
    return taskInfo;
  }

  /**
   * 停止指定会话的监控任务
   * 通过 sessionTaskMap 查找 taskId，调用后端停止并清理本地映射
   */
  async function stopMonitoring(sessionId: string) {
    const taskId = sessionTaskMap.value.get(sessionId);
    if (!taskId) return;
    await invoke('stop_monitoring', { taskId });
    const nextMap = new Map(sessionTaskMap.value);
    nextMap.delete(sessionId);
    sessionTaskMap.value = nextMap;
  }

  /**
   * 初始化监控事件监听器：
   * - monitor:snapshot：后端推送的监控快照
   * - task:status：长任务状态变更，写入 tasks 集合
   * 返回取消监听的清理函数
   */
  async function initListeners() {
    const unlistenSnapshot = await listen<MonitorSnapshot>('monitor:snapshot', (event) => {
      applySnapshot(event.payload);
    });

    const unlistenTask = await listen<TaskStatusEvent>('task:status', (event) => {
      applyTaskStatus(event.payload);
    });

    return () => {
      unlistenSnapshot();
      unlistenTask();
    };
  }

  return {
    snapshots,
    tasks,
    sessionTaskMap,
    activeSnapshot,
    getSessionTask,
    applySnapshot,
    applyTaskStatus,
    fetchSnapshot,
    startMonitoring,
    stopMonitoring,
    initListeners,
  };
});
