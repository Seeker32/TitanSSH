import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { create } from 'zustand';
import type { SessionInfo, SessionProgressEvent } from '@/types/session';
import { ConnectionPhase, SessionStatus } from '@/types/session';
import { useMonitorStore } from './monitor';
import { useSftpStore } from './sftp';

interface SessionStatusPayload {
  sessionId: string;
  status: SessionStatus;
  message?: string | null;
}

interface SessionState {
  sessions: Map<string, SessionInfo>;
  activeView: string | null;
  statusMessage: string;
  openSession: (hostId: string) => Promise<SessionInfo>;
  closeSession: (sessionId: string) => Promise<void>;
  writeTerminal: (sessionId: string, data: string) => Promise<void>;
  resizeTerminal: (sessionId: string, cols: number, rows: number) => Promise<void>;
  setActiveView: (viewId: string | null) => void;
  applySessionStatus: (payload: SessionStatusPayload) => void;
  applySessionProgress: (payload: SessionProgressEvent) => void;
  initListeners: () => Promise<() => void>;
}

/** 将连接状态映射为用户可读的中文提示。 */
export function statusLabel(status: SessionStatus, message?: string): string {
  switch (status) {
    case SessionStatus.Connecting: return '正在连接...';
    case SessionStatus.Connected: return '';
    case SessionStatus.AuthFailed: return '认证失败，请检查用户名和密码';
    case SessionStatus.Timeout: return '连接超时，请检查网络或主机地址';
    case SessionStatus.Error: return message?.trim() ? `连接错误：${message.trim()}` : '连接错误';
    case SessionStatus.Disconnected: return '连接已断开';
    default: return '连接异常';
  }
}

/** 将连接阶段映射为用户可读的中文进度。 */
export function progressLabel(phase: ConnectionPhase, message?: string): string {
  if (message?.trim()) return message.trim();
  switch (phase) {
    case ConnectionPhase.LoadingCredentials: return '正在读取凭据...';
    case ConnectionPhase.ConnectingTcp: return '正在建立 TCP 连接...';
    case ConnectionPhase.SshHandshake: return '正在进行 SSH 握手...';
    case ConnectionPhase.Authenticating: return '正在进行 SSH 认证...';
    case ConnectionPhase.OpeningChannel: return '正在打开终端通道...';
    case ConnectionPhase.RequestingPty: return '正在请求终端 PTY...';
    case ConnectionPhase.StartingShell: return '正在启动 Shell...';
    default: return '正在连接...';
  }
}

export const useSessionStore = create<SessionState>((set, get) => {
  return {
    sessions: new Map(),
    activeView: null,
    statusMessage: '就绪',

    /** 打开 SSH 会话，并启动关联监控任务。 */
    async openSession(hostId) {
      const session = await invoke<SessionInfo>('open_session', { hostId });
      set((state) => ({
        sessions: new Map(state.sessions).set(session.sessionId, session),
        activeView: session.sessionId,
        statusMessage: `正在连接 ${session.username}@${session.host}`,
      }));
      try {
        await useMonitorStore.getState().startMonitoring(session.sessionId);
      } catch {
        // 监控失败不阻断 SSH 主流程。
      }
      return session;
    },

    /** 关闭 SSH 会话；后端统一 teardown，前端只清理 projection。 */
    async closeSession(sessionId) {
      await invoke('close_session', { sessionId });
      set((state) => {
        const sessions = new Map(state.sessions);
        sessions.delete(sessionId);
        return { sessions, activeView: state.activeView === sessionId ? null : state.activeView };
      });
      useMonitorStore.getState().clearSession(sessionId);
      useSftpStore.getState().clearSession(sessionId);
    },

    /** 将用户输入写入指定终端会话。 */
    async writeTerminal(sessionId, data) {
      await invoke('write_terminal', { sessionId, data });
    },

    /** 将终端尺寸同步给后端 PTY。 */
    async resizeTerminal(sessionId, cols, rows) {
      await invoke('resize_terminal', { sessionId, cols, rows });
    },

    /** 切换当前会话视图；null 表示无会话（空态）。 */
    setActiveView(activeView) {
      set({ activeView });
    },

    /** 应用后端权威会话状态；连接成功时初始化该会话的远程目录。 */
    applySessionStatus(payload) {
      const current = get().sessions.get(payload.sessionId);
      set((state) => ({
        sessions: current
          ? new Map(state.sessions).set(payload.sessionId, { ...current, status: payload.status })
          : state.sessions,
        statusMessage: statusLabel(payload.status, payload.message ?? undefined),
      }));
      if (current && payload.status === SessionStatus.Connected) {
        useSftpStore.getState().listDir(payload.sessionId, '/').catch(() => {});
      }
    },

    /** 仅在连接中应用阶段诊断信息。 */
    applySessionProgress(payload) {
      const current = get().sessions.get(payload.sessionId);
      if (current?.status === SessionStatus.Connecting) {
        set({ statusMessage: progressLabel(payload.phase, payload.message) });
      }
    },

    /** 注册会话状态与连接进度事件，返回统一清理函数。 */
    async initListeners() {
      const unlistenStatus = await listen<SessionStatusPayload>('session:status', (event) => {
        get().applySessionStatus(event.payload);
      });
      const unlistenProgress = await listen<SessionProgressEvent>('session:progress', (event) => {
        get().applySessionProgress(event.payload);
      });
      return () => {
        unlistenStatus();
        unlistenProgress();
      };
    },
  };
});
