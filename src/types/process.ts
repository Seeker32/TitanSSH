/** 单个运行进程的结构化信息。 */
export interface ProcessInfo {
  pid: number;
  ppid: number;
  user: string;
  command: string;
  commandLine: string;
  cpuPercent: number | null;
  memoryBytes: number | null;
  state: string;
}

/** 单次全量进程采样结果。 */
export interface ProcessSnapshot {
  sessionId: string;
  timestamp: number;
  processes: ProcessInfo[];
  totalCount: number;
}
