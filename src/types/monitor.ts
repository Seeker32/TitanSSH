/** 监控快照，由后端一次性采集并推送，前端只渲染 */
export interface MonitorSnapshot {
  sessionId: string;
  /** Unix 毫秒时间戳 */
  timestamp: number;
  cpuUsage: number;
  memoryUsage: number;
  diskUsage: number;
  diskAvailableBytes: number;
  diskTotalBytes: number;
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
  taskId: string;
  taskType: string;
  sessionId?: string;
  status: TaskStatus;
  /** Unix 毫秒时间戳 */
  createdAt: number;
}

/** 长任务状态变更事件 payload */
export interface TaskStatusEvent {
  taskId: string;
  status: TaskStatus;
  message?: string;
}
