export interface RemoteEntry {
  name: string;
  path: string;
  isDir: boolean;
  size: number;
  /** Unix 毫秒时间戳 */
  modifiedAt: number;
  permissions: string;
}

export type TransferType = 'Upload' | 'Download';

/** 传输最终目标已存在时的冲突处理策略（上传与下载共用）；未显式指定时默认 Reject */
export type ConflictStrategy = 'Reject' | 'Overwrite';

/** 计算上传任务的远程目标目录；remotePath 为后端拼接的完整目标路径（目录/文件名）。 */
export function uploadTargetDir(task: Pick<TransferTask, 'remotePath' | 'fileName'>): string {
  const suffix = `/${task.fileName}`;
  return task.remotePath.endsWith(suffix) && task.remotePath.length > suffix.length
    ? task.remotePath.slice(0, -suffix.length)
    : '/';
}

/** SFTP 任务专用状态，Cancelled 区分主动取消与失败 */
export type SftpTaskStatus = 'Pending' | 'Running' | 'Done' | 'Failed' | 'Cancelled';

/** 传输任务；初始 status 为 Pending */
export interface TransferTask {
  taskId: string;
  sessionId: string;
  transferType: TransferType;
  remotePath: string;
  localPath: string;
  fileName: string;
  totalBytes: number;
  transferredBytes: number;
  speedBps: number;
  status: SftpTaskStatus;
  /** 失败原因；Failed 或取消后临时文件清理失败时为结构化错误，其余为 null */
  error: AppErrorInfo | null;
  /** Unix 毫秒时间戳 */
  createdAt: number;
}

export interface SftpProgressEvent {
  taskId: string;
  sessionId: string;
  transferredBytes: number;
  totalBytes: number;
  speedBps: number;
}

export interface SftpTaskStatusEvent {
  taskId: string;
  sessionId: string;
  status: SftpTaskStatus;
  error: AppErrorInfo | null;
}

/** sftp_task_snapshot 响应：指定 Session 的权威任务列表（createdAt 最新优先） */
export type SftpTaskSnapshot = TransferTask[];

/** 终态集合：不再接受任何状态迁移的任务状态（与后端 is_terminal 对齐） */
export const TERMINAL_TASK_STATUSES: readonly SftpTaskStatus[] = ['Done', 'Failed', 'Cancelled'];

/** 判断任务状态是否为终态。 */
export function isTerminalStatus(status: SftpTaskStatus): boolean {
  return TERMINAL_TASK_STATUSES.includes(status);
}

/** per-session SFTP 状态；selectedPaths 为运行时 Set，不序列化到 Tauri 边界 */
export interface SftpSessionState {
  currentPath: string;
  entries: RemoteEntry[];
  selectedPaths: Set<string>;
  loading: boolean;
  /** 文件浏览器级错误（目录列举、上传/下载/重试 invoke 拒绝） */
  error: AppErrorInfo | null;
  tasks: Map<string, TransferTask>;
  /** 任务行级操作错误（取消失败、重试失败）；键为 taskId，任务到达终态时清除 */
  taskActionErrors: Map<string, AppErrorInfo>;
  /** 本会话最新目录请求序号（单调递增）；旧请求据此判定为过期，不得更新投影 */
  dirRequestSeq: number;
}
import type { AppErrorInfo } from '@/i18n';
