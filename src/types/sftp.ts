export interface RemoteEntry {
  name: string;
  path: string;
  is_dir: boolean;
  size: number;
  /** Unix 毫秒时间戳 */
  modified_at: number;
  permissions: string;
}

export type TransferType = 'Upload' | 'Download';

/** SFTP 任务专用状态，Cancelled 区分主动取消与失败 */
export type SftpTaskStatus = 'Pending' | 'Running' | 'Done' | 'Failed' | 'Cancelled';

/** 传输任务；初始 status 为 Pending */
export interface TransferTask {
  task_id: string;
  session_id: string;
  transfer_type: TransferType;
  remote_path: string;
  local_path: string;
  file_name: string;
  total_bytes: number;
  transferred_bytes: number;
  speed_bps: number;
  status: SftpTaskStatus;
  /** 失败原因；Failed 时为错误描述，Cancelled 时为 null */
  error_message: string | null;
  /** Unix 毫秒时间戳 */
  created_at: number;
}

export interface SftpProgressEvent {
  task_id: string;
  session_id: string;
  transferred_bytes: number;
  total_bytes: number;
  speed_bps: number;
}

export interface SftpTaskStatusEvent {
  task_id: string;
  session_id: string;
  status: SftpTaskStatus;
  error_message: string | null;
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
