export interface SessionInfo {
  sessionId: string;
  hostId: string;
  host: string;
  port: number;
  username: string;
  status: SessionStatus;
  /** Unix 毫秒时间戳 */
  createdAt: number;
}

export enum ConnectionPhase {
  LoadingCredentials = "LoadingCredentials",
  ConnectingTcp = "ConnectingTcp",
  SshHandshake = "SshHandshake",
  Authenticating = "Authenticating",
  OpeningChannel = "OpeningChannel",
  RequestingPty = "RequestingPty",
  StartingShell = "StartingShell",
}

export interface SessionProgressEvent {
  sessionId: string;
  phase: ConnectionPhase;
  timestamp: number;
}

export interface SessionStatusEvent {
  sessionId: string;
  status: SessionStatus;
  error?: AppErrorInfo | null;
}

export enum SessionStatus {
  Connecting = "Connecting",
  Connected = "Connected",
  AuthFailed = "AuthFailed",
  Disconnected = "Disconnected",
  Timeout = "Timeout",
  Error = "Error",
}

/** 单个 Session 的连接生命周期投影：当前连接阶段与结构化失败原因；文案在渲染时按当前语言生成。 */
export interface SessionConnection {
  /** Connecting 阶段的当前进度；失败或无进度时为 null。 */
  phase: ConnectionPhase | null;
  /** 连接失败的结构化错误；Connecting/Connected 时为 null。 */
  error: AppErrorInfo | null;
}
import type { AppErrorInfo } from '@/i18n';
