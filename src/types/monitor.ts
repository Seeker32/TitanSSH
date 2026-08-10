/** 单张网卡接口的当前收发速率。 */
export interface NetworkInterface {
  name: string;
  /** 下行（RX）bytes/s；首次采样或计数异常时为 null。 */
  receiveBytesPerSecond: number | null;
  /** 上行（TX）bytes/s；首次采样或计数异常时为 null。 */
  transmitBytesPerSecond: number | null;
}

/** 网络采集状态，可区分不可用与成功但没有候选接口。 */
export interface NetworkSnapshot {
  available: boolean;
  interfaces: NetworkInterface[];
}

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
  network: NetworkSnapshot;
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
