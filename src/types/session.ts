export interface SessionInfo {
  session_id: string;
  host_id: string;
  host: string;
  port: number;
  username: string;
  status: SessionStatus;
  /** Unix 毫秒时间戳 */
  created_at: number;
}

export enum SessionStatus {
  Connecting = "Connecting",
  Connected = "Connected",
  AuthFailed = "AuthFailed",
  Disconnected = "Disconnected",
  Timeout = "Timeout",
  Error = "Error",
}
