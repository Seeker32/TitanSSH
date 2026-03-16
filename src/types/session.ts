export interface SessionInfo {
  session_id: string;
  host_id: string;
  host: string;
  port: number;
  username: string;
  status: SessionStatus;
  created_at: number;
  active: boolean;
  isHome?: boolean;
}

export enum SessionStatus {
  Connecting = "Connecting",
  Connected = "Connected",
  AuthFailed = "AuthFailed",
  Disconnected = "Disconnected",
  Timeout = "Timeout",
  Error = "Error",
}
