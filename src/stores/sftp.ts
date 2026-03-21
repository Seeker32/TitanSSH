import { defineStore } from 'pinia';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { ref, computed } from 'vue';
import type {
  SftpSessionState, RemoteEntry, TransferTask,
  SftpProgressEvent, SftpTaskStatusEvent,
} from '@/types/sftp';
import { useSessionStore } from './session';

export const useSftpStore = defineStore('sftp', () => {
  /** 按 session_id 索引的 per-session 状态 */
  const sessionStates = ref(new Map<string, SftpSessionState>());

  /** 当前激活 session 的 SFTP 状态，首页激活时返回 null */
  const activeState = computed(() => {
    const sessionStore = useSessionStore();
    if (sessionStore.activeView === 'home') return null;
    return sessionStates.value.get(sessionStore.activeView) ?? null;
  });

  /** 懒初始化指定 session 的状态，若已存在则直接返回 */
  function ensureState(sessionId: string): SftpSessionState {
    if (!sessionStates.value.has(sessionId)) {
      sessionStates.value = new Map(sessionStates.value).set(sessionId, {
        currentPath: '/',
        entries: [],
        selectedPaths: new Set(),
        loading: false,
        error: null,
        tasks: new Map(),
      });
    }
    return sessionStates.value.get(sessionId)!;
  }

  /** 获取指定 session 的状态，不存在时返回 undefined */
  function getState(sessionId: string): SftpSessionState | undefined {
    return sessionStates.value.get(sessionId);
  }

  /** 列举远程目录内容，更新 entries 和 currentPath */
  async function listDir(sessionId: string, path: string): Promise<void> {
    const state = ensureState(sessionId);
    state.loading = true;
    state.error = null;
    try {
      const entries = await invoke<RemoteEntry[]>('sftp_list_dir', { sessionId, path });
      state.entries = entries;
      state.currentPath = path;
      state.selectedPaths = new Set();
    } catch (e) {
      state.error = e instanceof Error ? e.message : String(e);
    } finally {
      state.loading = false;
    }
  }

  /** 发起下载任务，将返回的 TransferTask 写入 tasks */
  async function download(sessionId: string, remotePath: string, localPath: string): Promise<void> {
    const state = ensureState(sessionId);
    const task = await invoke<TransferTask>('sftp_download', { sessionId, remotePath, localPath });
    state.tasks = new Map(state.tasks).set(task.task_id, task);
  }

  /** 发起上传任务，将返回的 TransferTask 写入 tasks */
  async function upload(sessionId: string, localPath: string, remotePath: string): Promise<void> {
    const state = ensureState(sessionId);
    const task = await invoke<TransferTask>('sftp_upload', { sessionId, localPath, remotePath });
    state.tasks = new Map(state.tasks).set(task.task_id, task);
  }

  /** 取消指定传输任务 */
  async function cancelTask(taskId: string): Promise<void> {
    await invoke('sftp_cancel_task', { taskId });
  }

  /** 切换文件选中状态 */
  function toggleSelect(sessionId: string, path: string): void {
    const state = ensureState(sessionId);
    const next = new Set(state.selectedPaths);
    if (next.has(path)) {
      next.delete(path);
    } else {
      next.add(path);
    }
    state.selectedPaths = next;
  }

  /** 会话关闭时清理对应 session 的所有状态 */
  function clearSession(sessionId: string): void {
    const next = new Map(sessionStates.value);
    next.delete(sessionId);
    sessionStates.value = next;
  }

  /** 处理 sftp:progress 事件，终态任务忽略 */
  function applyProgress(event: SftpProgressEvent): void {
    const state = sessionStates.value.get(event.session_id);
    if (!state) return;
    const task = state.tasks.get(event.task_id);
    if (!task) return;
    const terminal = ['Done', 'Failed', 'Cancelled'] as const;
    if ((terminal as readonly string[]).includes(task.status)) return;
    state.tasks = new Map(state.tasks).set(event.task_id, {
      ...task,
      transferred_bytes: event.transferred_bytes,
      speed_bps: event.speed_bps,
    });
  }

  /** 处理 sftp:task_status 事件；Done 时强制 transferred_bytes = total_bytes */
  function applyTaskStatus(event: SftpTaskStatusEvent): void {
    const state = sessionStates.value.get(event.session_id);
    if (!state) return;
    const task = state.tasks.get(event.task_id);
    if (!task) return;
    const updated: TransferTask = {
      ...task,
      status: event.status,
      error_message: event.error_message,
    };
    if (event.status === 'Done') {
      updated.transferred_bytes = task.total_bytes;
    }
    state.tasks = new Map(state.tasks).set(event.task_id, updated);
  }

  /** 测试辅助：直接注入任务到指定 session（仅测试使用） */
  function _injectTask(sessionId: string, task: TransferTask): void {
    const state = ensureState(sessionId);
    state.tasks = new Map(state.tasks).set(task.task_id, task);
  }

  /** 注册 sftp:progress 和 sftp:task_status 事件监听器，返回清理函数 */
  async function initListeners(): Promise<() => void> {
    const unlistenProgress = await listen<SftpProgressEvent>('sftp:progress', (e) => {
      applyProgress(e.payload);
    });
    const unlistenStatus = await listen<SftpTaskStatusEvent>('sftp:task_status', (e) => {
      applyTaskStatus(e.payload);
    });
    return () => {
      unlistenProgress();
      unlistenStatus();
    };
  }

  return {
    sessionStates,
    activeState,
    getState,
    ensureState,
    listDir,
    download,
    upload,
    cancelTask,
    toggleSelect,
    clearSession,
    applyProgress,
    applyTaskStatus,
    initListeners,
    _injectTask,
  };
});
