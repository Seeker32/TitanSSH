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
  /** 失败原因；Failed 时为错误描述，Cancelled 时为 null */
  errorMessage: string | null;
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
  errorMessage: string | null;
}

/** per-session SFTP 状态；selectedPaths 为运行时 Set，不序列化到 Tauri 边界 */
export interface SftpSessionState {
  currentPath: string;
  entries: RemoteEntry[];
  selectedPaths: Set<string>;
  loading: boolean;
  error: string | null;
  tasks: Map<string, TransferTask>;
}
