/** 监控快照，由后端一次性采集并推送，前端只渲染 */
export interface MonitorSnapshot {
  session_id: string;
  /** Unix 毫秒时间戳 */
  timestamp: number;
  cpu_usage: number;
  memory_usage: number;
  disk_usage: number;
}

/** 长任务状态枚举 */
export enum TaskStatus {
  Pending = 'Pending',
  Running = 'Running',
  Done = 'Done',
  Failed = 'Failed',
}

/** 长任务信息，所有持续任务必须具备 taskId 与状态 */
export interface TaskInfo {
  task_id: string;
  task_type: string;
  session_id?: string;
  status: TaskStatus;
  /** Unix 毫秒时间戳 */
  created_at: number;
}

/** 长任务状态变更事件 payload */
export interface TaskStatusEvent {
  task_id: string;
  status: TaskStatus;
  message?: string;
}
