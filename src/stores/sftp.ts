import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { create } from 'zustand';
import { toAppError, type AppErrorInfo } from '@/i18n';
import { isTerminalStatus, uploadTargetDir } from '@/types/sftp';
import type {
  ConflictStrategy,
  RemoteEntry,
  SftpProgressEvent,
  SftpSessionState,
  SftpTaskSnapshot,
  SftpTaskStatusEvent,
  TransferTask,
} from '@/types/sftp';

interface SftpState {
  sessionStates: Map<string, SftpSessionState>;
  /** invoke 返回前到达的任务状态事件缓存，任务元数据到达后补投 */
  pendingTaskEvents: Map<string, SftpTaskStatusEvent>;
  /** 已关闭会话 ID：清理后迟到的状态事件直接丢弃，不再落回缓存 */
  closedSessions: Set<string>;
  getState: (sessionId: string) => SftpSessionState | undefined;
  ensureState: (sessionId: string) => SftpSessionState;
  listDir: (sessionId: string, path: string) => Promise<void>;
  loadTaskSnapshot: (sessionId: string) => Promise<void>;
  download: (sessionId: string, remotePath: string, localPath: string, parentTaskId?: string, conflictStrategy?: ConflictStrategy) => Promise<void>;
  upload: (sessionId: string, localPath: string, remotePath: string, parentTaskId?: string, conflictStrategy?: ConflictStrategy) => Promise<void>;
  cancelTask: (taskId: string, sessionId: string) => Promise<void>;
  clearTerminalTasks: (sessionId: string) => Promise<void>;
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
  closedSessions: new Set(),

  /** 获取指定会话状态；不存在时返回 undefined。 */
  getState(sessionId) {
    return get().sessionStates.get(sessionId);
  },

  /** 懒初始化并返回指定会话状态；重新打开已关闭会话时撤下关闭标记。 */
  ensureState(sessionId) {
    const existing = get().sessionStates.get(sessionId);
    if (existing) return existing;
    const created = emptySessionState();
    set((state) => {
      const closedSessions = state.closedSessions.has(sessionId)
        ? new Set([...state.closedSessions].filter((id) => id !== sessionId))
        : state.closedSessions;
      return {
        sessionStates: new Map(state.sessionStates).set(sessionId, created),
        closedSessions,
      };
    });
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

  /** 用后端权威快照重建指定会话的任务投影（恢复错过的事件），并补投
   *  快照返回前到达、被缓存的早到状态事件；后续事件继续增量更新。
   *  invoke 拒绝不向外抛出，错误写入文件浏览器错误区。 */
  async loadTaskSnapshot(sessionId) {
    const startedAt = Date.now(); // 快照请求开始时间：其后入队的任务比快照更新
    try {
      const tasks = await invoke<SftpTaskSnapshot>('sftp_task_snapshot', { sessionId });
      updateSession(set, sessionId, (state) => {
        const merged = new Map((tasks ?? []).map((task) => [task.taskId, task]));
        // 快照采集时刻早于本次 invoke：请求开始后入队的任务本地投影更新，覆盖快照旧状态
        for (const [taskId, task] of state.tasks) {
          if (task.createdAt >= startedAt) merged.set(taskId, task);
        }
        return { ...state, tasks: merged };
      });
      // 会话重新打开：撤下关闭标记，后续事件恢复增量更新
      if (get().closedSessions.has(sessionId)) {
        set((state) => ({
          closedSessions: new Set([...state.closedSessions].filter((id) => id !== sessionId)),
        }));
      }
      // 补投快照返回前到达的早到事件；未知任务的事件补投失败后会重新落回缓存
      for (const event of [...get().pendingTaskEvents.values()]) {
        if (event.sessionId === sessionId) get().applyBufferedTaskStatus(event.taskId);
      }
    } catch (error) {
      updateSession(set, sessionId, (state) => ({ ...state, error: toAppError(error) }));
    }
  },

  /** 发起下载任务并写入对应会话的任务队列；补投 invoke 返回前到达的事件。
   *  冲突策略默认 Reject；仅当用户对单个冲突文件确认覆盖时才传 Overwrite，
   *  确认不扩展到批次或 Session。invoke 拒绝（启动失败）不向外抛出：重试场景
   *  （parentTaskId）只在原任务行标注 actionError，否则写入文件浏览器错误区；
   *  成功后清除原任务行的旧操作错误。 */
  async download(sessionId, remotePath, localPath, parentTaskId, conflictStrategy = 'Reject') {
    try {
      const task = await invoke<TransferTask>('sftp_download', {
        sessionId, remotePath, localPath, conflictStrategy,
      });
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
   *  冲突策略默认 Reject；仅当用户对单个冲突文件确认覆盖时才传 Overwrite，
   *  确认不扩展到批次或 Session。invoke 拒绝（启动失败）不向外抛出：重试场景
   *  （parentTaskId）只在原任务行标注 actionError，否则写入文件浏览器错误区；
   *  成功后清除原任务行的旧操作错误。 */
  async upload(sessionId, localPath, remotePath, parentTaskId, conflictStrategy = 'Reject') {
    try {
      const task = await invoke<TransferTask>('sftp_upload', {
        sessionId, localPath, remotePath, conflictStrategy,
      });
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

  /** 清除指定会话的全部终态任务：后端权威清除成功后同步本地投影，
   *  Pending/Running 活动任务保留，并清理被移除任务行的 actionError。
   *  invoke 拒绝不向外抛出，错误写入文件浏览器错误区。 */
  async clearTerminalTasks(sessionId) {
    try {
      await invoke('sftp_clear_terminal_tasks', { sessionId });
      updateSession(set, sessionId, (state) => {
        const tasks = new Map(state.tasks);
        const taskActionErrors = new Map(state.taskActionErrors);
        for (const [taskId, task] of tasks) {
          if (isTerminalStatus(task.status)) {
            tasks.delete(taskId);
            taskActionErrors.delete(taskId);
          }
        }
        return { ...state, tasks, taskActionErrors };
      });
    } catch (error) {
      updateSession(set, sessionId, (state) => ({ ...state, error: toAppError(error) }));
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

  /** 清理已关闭会话的全部 SFTP 状态与同会话的缓存事件，并登记关闭标记；
   *  此后到达的迟到状态事件直接丢弃，不再落回缓存。 */
  clearSession(sessionId) {
    set((state) => {
      const sessionStates = new Map(state.sessionStates);
      sessionStates.delete(sessionId);
      const pendingTaskEvents = new Map(state.pendingTaskEvents);
      for (const [taskId, event] of pendingTaskEvents) {
        if (event.sessionId === sessionId) pendingTaskEvents.delete(taskId);
      }
      return {
        sessionStates,
        pendingTaskEvents,
        closedSessions: new Set(state.closedSessions).add(sessionId),
      };
    });
  },

  /** 应用传输进度；终态任务不允许进度回退。 */
  applyProgress(event) {
    const state = get().sessionStates.get(event.sessionId);
    const task = state?.tasks.get(event.taskId);
    if (!state || !task || isTerminalStatus(task.status)) return;
    updateSession(set, event.sessionId, (current) => ({
      ...current,
      tasks: new Map(current.tasks).set(event.taskId, {
        ...task, transferredBytes: event.transferredBytes, speedBps: event.speedBps,
      }),
    }));
  },

  /** 应用传输任务终态；完成时强制进度为总大小。未知任务缓存最新事件。
   *  会话已关闭（投影已清空）时迟到事件直接丢弃；任务到达终态时同步清除
   *  对应任务行的 actionError（取消失败等操作错误已失去意义）。
   *  上传 Done 且用户仍停留在目标目录时自动刷新该目录（离开目标目录的用户
   *  不会被拉回）；刷新经 listDir 的请求序号机制服从最新目录请求规则。 */
  applyTaskStatus(event) {
    if (get().closedSessions.has(event.sessionId)) return;
    const state = get().sessionStates.get(event.sessionId);
    const task = state?.tasks.get(event.taskId);
    if (!task) {
      set((state) => ({
        pendingTaskEvents: new Map(state.pendingTaskEvents).set(event.taskId, event),
      }));
      return;
    }
    updateSession(set, event.sessionId, (current) => {
      const taskActionErrors = isTerminalStatus(event.status)
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
    // 仅上传完成且用户仍位于目标目录时刷新：离开目标目录的用户不得被强制导航回来
    if (event.status === 'Done' && task.transferType === 'Upload') {
      const targetDir = uploadTargetDir(task);
      if (state?.currentPath === targetDir) {
        void get().listDir(event.sessionId, targetDir);
      }
    }
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
