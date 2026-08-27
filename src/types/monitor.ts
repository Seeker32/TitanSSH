/** 单张网卡接口的当前收发速率。 */
export interface NetworkInterface {
  name: string;
  /** 下行（RX）bytes/s；首次采样或计数异常时为 null。 */
  receiveBytesPerSecond: number | null;
  /** 上行（TX）bytes/s；首次采样或计数异常时为 null。 */
  transmitBytesPerSecond: number | null;
}

/** 所选网卡在一个监控快照时刻的双向速率样本。 */
export interface NetworkTrendSample {
  /** 监控快照的 Unix 毫秒时间戳。 */
  timestamp: number;
  /** 下行速率；null 用于展示真实的数据缺口。 */
  receiveBytesPerSecond: number | null;
  /** 上行速率；null 用于展示真实的数据缺口。 */
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
  /** 0.0 ~ 100.0；无基线或采集缺失时为 null（未知） */
  cpuUsage: number | null;
  /** 0.0 ~ 100.0；MemTotal/MemAvailable 缺失时为 null（未知） */
  memoryUsage: number | null;
  /** 内存总容量（字节）；后端未上报或采集缺失时为 null */
  memoryTotalBytes: number | null;
  /** 内存已用量（字节）；后端未上报或采集缺失时为 null */
  memoryUsedBytes: number | null;
  /** 0.0 ~ 100.0；df 采集失败时为 null（未知） */
  diskUsage: number | null;
  /** 根分区剩余容量；df 采集失败时为 null */
  diskAvailableBytes: number | null;
  /** 根分区总容量；df 采集失败时为 null */
  diskTotalBytes: number | null;
  network: NetworkSnapshot;
}

/** 长任务状态枚举 */
export enum TaskStatus {
  Pending = 'Pending',
  Running = 'Running',
  Done = 'Done',
  Failed = 'Failed',
}

/** 前端共享投影支持的采样任务类型。 */
export type SamplingTaskType = 'monitor' | 'process';

/** 长任务信息，所有持续任务必须具备 taskId 与状态 */
export interface TaskInfo {
  taskId: string;
  taskType: SamplingTaskType;
  sessionId?: string;
  status: TaskStatus;
  /** Unix 毫秒时间戳 */
  createdAt: number;
  /** 最近的失败详情；仅 Failed 状态可能存在。 */
  error?: AppErrorInfo | null;
}

/** 长任务状态变更事件 payload */
export interface TaskStatusEvent {
  taskId: string;
  taskType: SamplingTaskType;
  sessionId: string;
  status: TaskStatus;
  error?: AppErrorInfo | null;
}
import type { AppErrorInfo } from '@/i18n';
