import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { create } from 'zustand';
import { toAppError } from '@/i18n';
import type {
  RemoteEntry,
  SftpProgressEvent,
  SftpSessionState,
  SftpTaskStatusEvent,
  TransferTask,
} from '@/types/sftp';

interface SftpState {
  sessionStates: Map<string, SftpSessionState>;
  /** invoke 返回前到达的任务状态事件缓存，任务元数据到达后补投 */
  pendingTaskEvents: Map<string, SftpTaskStatusEvent>;
  getState: (sessionId: string) => SftpSessionState | undefined;
  ensureState: (sessionId: string) => SftpSessionState;
  listDir: (sessionId: string, path: string) => Promise<void>;
  download: (sessionId: string, remotePath: string, localPath: string) => Promise<void>;
  upload: (sessionId: string, localPath: string, remotePath: string) => Promise<void>;
  cancelTask: (taskId: string) => Promise<void>;
  toggleSelect: (sessionId: string, path: string) => void;
  clearSession: (sessionId: string) => void;
  applyProgress: (event: SftpProgressEvent) => void;
  applyTaskStatus: (event: SftpTaskStatusEvent) => void;
  applyBufferedTaskStatus: (taskId: string) => void;
  initListeners: () => Promise<() => void>;
}

/** 创建指定会话的空 SFTP 状态。 */
function emptySessionState(): SftpSessionState {
  return {
    currentPath: '/', entries: [], selectedPaths: new Set(), loading: false, error: null, tasks: new Map(),
  };
}

/** 不可变更新指定会话状态，确保 Zustand 能通知订阅者。 */
function updateSession(
  set: (updater: (state: SftpState) => Partial<SftpState>) => void,
  sessionId: string,
  update: (state: SftpSessionState) => SftpSessionState,
) {
  set((state) => {
    const current = state.sessionStates.get(sessionId) ?? emptySessionState();
    return { sessionStates: new Map(state.sessionStates).set(sessionId, update(current)) };
  });
}

export const useSftpStore = create<SftpState>((set, get) => ({
  sessionStates: new Map(),
  pendingTaskEvents: new Map(),

  /** 获取指定会话状态；不存在时返回 undefined。 */
  getState(sessionId) {
    return get().sessionStates.get(sessionId);
  },

  /** 懒初始化并返回指定会话状态。 */
  ensureState(sessionId) {
    const existing = get().sessionStates.get(sessionId);
    if (existing) return existing;
    const created = emptySessionState();
    set((state) => ({ sessionStates: new Map(state.sessionStates).set(sessionId, created) }));
    return created;
  },

  /** 列举远程目录并更新当前路径、条目与错误状态。 */
  async listDir(sessionId, path) {
    updateSession(set, sessionId, (state) => ({ ...state, loading: true, error: null }));
    try {
      const entries = await invoke<RemoteEntry[]>('sftp_list_dir', { sessionId, path });
      updateSession(set, sessionId, (state) => ({
        ...state, entries: Array.isArray(entries) ? entries : [], currentPath: path, selectedPaths: new Set(),
      }));
    } catch (error) {
      updateSession(set, sessionId, (state) => ({
        ...state, error: toAppError(error),
      }));
    } finally {
      updateSession(set, sessionId, (state) => ({ ...state, loading: false }));
    }
  },

  /** 发起下载任务并写入对应会话的任务队列；补投 invoke 返回前到达的事件。 */
  async download(sessionId, remotePath, localPath) {
    const task = await invoke<TransferTask>('sftp_download', { sessionId, remotePath, localPath });
    updateSession(set, sessionId, (state) => ({ ...state, tasks: new Map(state.tasks).set(task.taskId, task) }));
    get().applyBufferedTaskStatus(task.taskId);
  },

  /** 发起上传任务并写入对应会话的任务队列；补投 invoke 返回前到达的事件。 */
  async upload(sessionId, localPath, remotePath) {
    const task = await invoke<TransferTask>('sftp_upload', { sessionId, localPath, remotePath });
    updateSession(set, sessionId, (state) => ({ ...state, tasks: new Map(state.tasks).set(task.taskId, task) }));
    get().applyBufferedTaskStatus(task.taskId);
  },

  /** 取消指定传输任务。 */
  async cancelTask(taskId) {
    await invoke('sftp_cancel_task', { taskId });
  },

  /** 切换指定远程路径的选中状态。 */
  toggleSelect(sessionId, path) {
    updateSession(set, sessionId, (state) => {
      const selectedPaths = new Set(state.selectedPaths);
      selectedPaths.has(path) ? selectedPaths.delete(path) : selectedPaths.add(path);
      return { ...state, selectedPaths };
    });
  },

  /** 清理已关闭会话的全部 SFTP 状态与同会话的缓存事件。 */
  clearSession(sessionId) {
    set((state) => {
      const sessionStates = new Map(state.sessionStates);
      sessionStates.delete(sessionId);
      const pendingTaskEvents = new Map(state.pendingTaskEvents);
      for (const [taskId, event] of pendingTaskEvents) {
        if (event.sessionId === sessionId) pendingTaskEvents.delete(taskId);
      }
      return { sessionStates, pendingTaskEvents };
    });
  },

  /** 应用传输进度；终态任务不允许进度回退。 */
  applyProgress(event) {
    const state = get().sessionStates.get(event.sessionId);
    const task = state?.tasks.get(event.taskId);
    if (!state || !task || ['Done', 'Failed', 'Cancelled'].includes(task.status)) return;
    updateSession(set, event.sessionId, (current) => ({
      ...current,
      tasks: new Map(current.tasks).set(event.taskId, {
        ...task, transferredBytes: event.transferredBytes, speedBps: event.speedBps,
      }),
    }));
  },

  /** 应用传输任务终态；完成时强制进度为总大小。未知任务缓存最新事件。 */
  applyTaskStatus(event) {
    const state = get().sessionStates.get(event.sessionId);
    const task = state?.tasks.get(event.taskId);
    if (!task) {
      set((state) => ({
        pendingTaskEvents: new Map(state.pendingTaskEvents).set(event.taskId, event),
      }));
      return;
    }
    updateSession(set, event.sessionId, (current) => ({
      ...current,
      tasks: new Map(current.tasks).set(event.taskId, {
        ...task,
        status: event.status,
        error: event.error,
        transferredBytes: event.status === 'Done' ? task.totalBytes : task.transferredBytes,
      }),
    }));
  },

  /** 任务元数据到达后补投缓存的状态事件；事件不再落回缓存。 */
  applyBufferedTaskStatus(taskId: string) {
    const buffered = get().pendingTaskEvents.get(taskId);
    if (!buffered) return;
    set((state) => {
      const pendingTaskEvents = new Map(state.pendingTaskEvents);
      pendingTaskEvents.delete(taskId);
      return { pendingTaskEvents };
    });
    get().applyTaskStatus(buffered);
  },

  /** 注册 SFTP 进度和任务状态事件，返回清理函数。 */
  async initListeners() {
    const unlistenProgress = await listen<SftpProgressEvent>('sftp:progress', (event) => {
      get().applyProgress(event.payload);
    });
    const unlistenStatus = await listen<SftpTaskStatusEvent>('sftp:task_status', (event) => {
      get().applyTaskStatus(event.payload);
    });
    return () => {
      unlistenProgress();
      unlistenStatus();
    };
  },
}));
