import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { create } from 'zustand';
import type { MonitorSnapshot } from '@/types/monitor';
import type { SessionInfo, SessionProgressEvent } from '@/types/session';
import { ConnectionPhase, SessionStatus } from '@/types/session';
import { useMonitorStore } from './monitor';
import { useSftpStore } from './sftp';

const CONNECT_WATCHDOG_MS = 15_000;
const connectWatchdogs = new Map<string, ReturnType<typeof setTimeout>>();

interface SessionStatusPayload {
  sessionId: string;
  status: SessionStatus;
  message?: string | null;
}

interface TerminalDataPayload {
  sessionId: string;
  data: string;
}

interface SessionState {
  sessions: Map<string, SessionInfo>;
  activeView: 'home' | string;
  snapshots: Map<string, MonitorSnapshot>;
  statusMessage: string;
  openSession: (hostId: string) => Promise<SessionInfo>;
  closeSession: (sessionId: string) => Promise<void>;
  writeTerminal: (sessionId: string, data: string) => Promise<void>;
  resizeTerminal: (sessionId: string, cols: number, rows: number) => Promise<void>;
  setActiveView: (viewId: 'home' | string) => void;
  applySessionStatus: (payload: SessionStatusPayload) => void;
  applySessionProgress: (payload: SessionProgressEvent) => void;
  applySnapshot: (snapshot: MonitorSnapshot) => void;
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
  /** 清理指定会话的连接 watchdog。 */
  function clearWatchdog(sessionId: string) {
    const timer = connectWatchdogs.get(sessionId);
    if (!timer) return;
    clearTimeout(timer);
    connectWatchdogs.delete(sessionId);
  }

  /** 注册指定会话的连接超时 watchdog。 */
  function scheduleWatchdog(sessionId: string) {
    clearWatchdog(sessionId);
    connectWatchdogs.set(sessionId, setTimeout(() => {
      const session = get().sessions.get(sessionId);
      if (session?.status === SessionStatus.Connecting) {
        get().applySessionStatus({
          sessionId,
          status: SessionStatus.Timeout,
          message: `Connection watchdog timeout after ${CONNECT_WATCHDOG_MS / 1000}s`,
        });
      }
    }, CONNECT_WATCHDOG_MS));
  }

  return {
    sessions: new Map(),
    activeView: 'home',
    snapshots: new Map(),
    statusMessage: '就绪',

    /** 打开 SSH 会话，并启动关联监控任务。 */
    async openSession(hostId) {
      const session = await invoke<SessionInfo>('open_session', { hostId });
      set((state) => ({
        sessions: new Map(state.sessions).set(session.sessionId, session),
        activeView: session.sessionId,
        statusMessage: `正在连接 ${session.username}@${session.host}`,
      }));
      scheduleWatchdog(session.sessionId);
      try {
        await useMonitorStore.getState().startMonitoring(session.sessionId);
      } catch {
        // 监控失败不阻断 SSH 主流程。
      }
      return session;
    },

    /** 关闭 SSH 会话，并停止监控及清理关联前端状态。 */
    async closeSession(sessionId) {
      clearWatchdog(sessionId);
      try {
        await useMonitorStore.getState().stopMonitoring(sessionId);
      } catch {
        // 监控停止失败不阻断会话关闭。
      }
      await invoke('close_session', { sessionId });
      set((state) => {
        const sessions = new Map(state.sessions);
        const snapshots = new Map(state.snapshots);
        sessions.delete(sessionId);
        snapshots.delete(sessionId);
        return { sessions, snapshots, activeView: state.activeView === sessionId ? 'home' : state.activeView };
      });
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

    /** 切换首页或真实会话视图。 */
    setActiveView(activeView) {
      set({ activeView });
    },

    /** 应用会话状态事件，并同步后端会话元数据。 */
    applySessionStatus(payload) {
      const current = get().sessions.get(payload.sessionId);
      if (payload.status !== SessionStatus.Connecting) clearWatchdog(payload.sessionId);
      set((state) => ({
        sessions: current
          ? new Map(state.sessions).set(payload.sessionId, { ...current, status: payload.status })
          : state.sessions,
        statusMessage: statusLabel(payload.status, payload.message ?? undefined),
      }));
      Promise.resolve(invoke('sync_session_status', {
        sessionId: payload.sessionId,
        status: payload.status,
      })).catch(() => {});
    },

    /** 仅在连接中应用阶段诊断信息。 */
    applySessionProgress(payload) {
      const current = get().sessions.get(payload.sessionId);
      if (current?.status === SessionStatus.Connecting) {
        set({ statusMessage: progressLabel(payload.phase, payload.message) });
      }
    },

    /** 保存指定会话的监控快照兼容缓存。 */
    applySnapshot(snapshot) {
      set((state) => ({ snapshots: new Map(state.snapshots).set(snapshot.sessionId, snapshot) }));
    },

    /** 注册会话、终端和监控事件，返回统一清理函数。 */
    async initListeners() {
      const unlistenStatus = await listen<SessionStatusPayload>('session:status', (event) => {
        get().applySessionStatus(event.payload);
      });
      const unlistenProgress = await listen<SessionProgressEvent>('session:progress', (event) => {
        get().applySessionProgress(event.payload);
      });
      const unlistenData = await listen<TerminalDataPayload>('terminal:data', () => {});
      const unlistenSnapshot = await listen<MonitorSnapshot>('monitor:snapshot', (event) => {
        get().applySnapshot(event.payload);
      });
      return () => {
        unlistenStatus();
        unlistenProgress();
        unlistenData();
        unlistenSnapshot();
      };
    },
  };
});
