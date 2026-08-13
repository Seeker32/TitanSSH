import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { create } from 'zustand';
import { toAppError, type AppErrorInfo } from '@/i18n';
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
  download: (sessionId: string, remotePath: string, localPath: string, parentTaskId?: string) => Promise<void>;
  upload: (sessionId: string, localPath: string, remotePath: string, parentTaskId?: string) => Promise<void>;
  cancelTask: (taskId: string, sessionId: string) => Promise<void>;
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
    taskActionErrors: new Map(), dirRequestSeq: 0,
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

/** 清除指定任务的操作错误；无 taskId 或不存在时原样返回。 */
function clearActionError(
  taskActionErrors: Map<string, AppErrorInfo>,
  taskId: string | undefined,
): Map<string, AppErrorInfo> {
  if (!taskId || !taskActionErrors.has(taskId)) return taskActionErrors;
  const next = new Map(taskActionErrors);
  next.delete(taskId);
  return next;
}

/** 记录传输启动 invoke 拒绝：重试场景写原任务行 actionError，否则写文件浏览器错误区。 */
function recordStartError(
  set: (updater: (state: SftpState) => Partial<SftpState>) => void,
  sessionId: string,
  parentTaskId: string | undefined,
  error: unknown,
) {
  const appError = toAppError(error);
  updateSession(set, sessionId, (state) => parentTaskId
    ? { ...state, taskActionErrors: new Map(state.taskActionErrors).set(parentTaskId, appError) }
    : { ...state, error: appError });
}

/** 递增指定会话的目录请求序号并置 loading；返回本次请求序号。
 *  Zustand 的 set 同步应用 updater，返回前序号已落库，后续异步结果据此判断新旧。 */
function startDirRequest(
  set: (updater: (state: SftpState) => Partial<SftpState>) => void,
  sessionId: string,
): number {
  let requestSeq = 0;
  set((state) => {
    const current = state.sessionStates.get(sessionId) ?? emptySessionState();
    requestSeq = current.dirRequestSeq + 1;
    return {
      sessionStates: new Map(state.sessionStates).set(sessionId, {
        ...current, dirRequestSeq: requestSeq, loading: true, error: null,
      }),
    };
  });
  return requestSeq;
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

  /** 列举远程目录并更新当前路径、条目与错误状态。
   *  同会话请求携带单调递增序号：只有最新请求可更新 entries、currentPath、error 与
   *  loading，旧请求完成或失败不得让投影倒退或提前结束最新请求的加载。 */
  async listDir(sessionId, path) {
    const requestSeq = startDirRequest(set, sessionId);
    try {
      const entries = await invoke<RemoteEntry[]>('sftp_list_dir', { sessionId, path });
      updateSession(set, sessionId, (state) => {
        if (state.dirRequestSeq !== requestSeq) return state; // 旧请求不得覆盖最新投影
        return {
          ...state, entries: Array.isArray(entries) ? entries : [], currentPath: path,
          selectedPaths: new Set(), loading: false,
        };
      });
    } catch (error) {
      updateSession(set, sessionId, (state) => {
        if (state.dirRequestSeq !== requestSeq) return state; // 旧请求失败不得结束最新 loading
        return { ...state, error: toAppError(error), loading: false };
      });
    }
  },

  /** 发起下载任务并写入对应会话的任务队列；补投 invoke 返回前到达的事件。
   *  invoke 拒绝（启动失败）不向外抛出：重试场景（parentTaskId）只在原任务行
   *  标注 actionError，否则写入文件浏览器错误区；成功后清除原任务行的旧操作错误。 */
  async download(sessionId, remotePath, localPath, parentTaskId) {
    try {
      const task = await invoke<TransferTask>('sftp_download', { sessionId, remotePath, localPath });
      updateSession(set, sessionId, (state) => ({
        ...state,
        tasks: new Map(state.tasks).set(task.taskId, task),
        taskActionErrors: clearActionError(state.taskActionErrors, parentTaskId),
      }));
      get().applyBufferedTaskStatus(task.taskId);
    } catch (error) {
      recordStartError(set, sessionId, parentTaskId, error);
    }
  },

  /** 发起上传任务并写入对应会话的任务队列；补投 invoke 返回前到达的事件。
   *  invoke 拒绝（启动失败）不向外抛出：重试场景（parentTaskId）只在原任务行
   *  标注 actionError，否则写入文件浏览器错误区；成功后清除原任务行的旧操作错误。 */
  async upload(sessionId, localPath, remotePath, parentTaskId) {
    try {
      const task = await invoke<TransferTask>('sftp_upload', { sessionId, localPath, remotePath });
      updateSession(set, sessionId, (state) => ({
        ...state,
        tasks: new Map(state.tasks).set(task.taskId, task),
        taskActionErrors: clearActionError(state.taskActionErrors, parentTaskId),
      }));
      get().applyBufferedTaskStatus(task.taskId);
    } catch (error) {
      recordStartError(set, sessionId, parentTaskId, error);
    }
  },

  /** 取消指定传输任务；invoke 拒绝（取消失败）在对应任务行标注 actionError。 */
  async cancelTask(taskId, sessionId) {
    try {
      await invoke('sftp_cancel_task', { taskId });
    } catch (error) {
      const appError = toAppError(error);
      updateSession(set, sessionId, (state) => ({
        ...state,
        taskActionErrors: new Map(state.taskActionErrors).set(taskId, appError),
      }));
    }
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

  /** 应用传输任务终态；完成时强制进度为总大小。未知任务缓存最新事件。
   *  任务到达终态时同步清除对应任务行的 actionError（取消失败等操作错误已失去意义）。 */
  applyTaskStatus(event) {
    const state = get().sessionStates.get(event.sessionId);
    const task = state?.tasks.get(event.taskId);
    if (!task) {
      set((state) => ({
        pendingTaskEvents: new Map(state.pendingTaskEvents).set(event.taskId, event),
      }));
      return;
    }
    updateSession(set, event.sessionId, (current) => {
      const taskActionErrors = ['Done', 'Failed', 'Cancelled'].includes(event.status)
        ? (() => {
            const next = new Map(current.taskActionErrors);
            next.delete(event.taskId);
            return next;
          })()
        : current.taskActionErrors;
      return {
        ...current,
        tasks: new Map(current.tasks).set(event.taskId, {
          ...task,
          status: event.status,
          error: event.error,
          transferredBytes: event.status === 'Done' ? task.totalBytes : task.transferredBytes,
        }),
        taskActionErrors,
      };
    });
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
