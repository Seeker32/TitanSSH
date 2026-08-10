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
import type { AppErrorInfo } from '@/i18n';
