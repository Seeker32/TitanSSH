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
  VerifyingHostKey = "VerifyingHostKey",
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

/** 首次未知主机身份确认事件（host-identity:challenge）；指纹由后端计算，前端不解析 SSH key 文本。
 *  kind=Changed 时同时携带已保存旧记录与服务器本次呈现的算法/指纹。 */
export interface HostIdentityChallenge {
  challengeId: string;
  sessionId: string;
  host: string;
  port: number;
  /** challenge 类型：未知主机或已保存 key 与呈现不一致；旧后端缺省视为 Unknown。 */
  kind?: HostIdentityChallengeKind;
  /** OpenSSH 风格算法名（如 ssh-ed25519）；Changed 时为服务器本次呈现的算法 */
  keyAlgorithm: string;
  /** OpenSSH 风格 SHA-256 指纹；Changed 时为服务器本次呈现 key 的指纹 */
  fingerprint: string;
  /** Changed 专属：已保存信任记录的算法名；Unknown 为 null/缺省 */
  storedAlgorithm?: string | null;
  /** Changed 专属：已保存信任记录的 SHA-256 指纹；Unknown 为 null/缺省 */
  storedFingerprint?: string | null;
  /** Unix 毫秒时间戳 */
  timestamp: number;
}

/** challenge 类型：未知主机 / 已保存记录与呈现 key 不一致（主机指纹变化） */
export type HostIdentityChallengeKind = 'Unknown' | 'Changed';

/** 后端撤销未决挑战的通知（host-identity:challenge-dismissed）：
 *  被新指纹取代、会话关闭、异地解决或应用退出时派发，前端据此撤下确认卡。 */
export interface HostIdentityChallengeDismissed {
  challengeId: string;
  sessionId: string;
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
