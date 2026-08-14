import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { create } from 'zustand';
import type { SessionConnection, SessionInfo, SessionProgressEvent, SessionStatusEvent } from '@/types/session';
import { ConnectionPhase, SessionStatus } from '@/types/session';
import type { AppErrorInfo, Locale, TranslationKey } from '@/i18n';
import { formatAppError, translate } from '@/i18n';
import { useLocaleStore } from '@/stores/locale';
import { useMonitorStore } from './monitor';
import { useSftpStore } from './sftp';

interface SessionState {
  sessions: Map<string, SessionInfo>;
  activeView: string | null;
  /** 按 sessionId 存储的连接生命周期投影（阶段 + 结构化错误）；Connected/Disconnected 后清除。 */
  connections: Map<string, SessionConnection>;
  openSession: (hostId: string) => Promise<SessionInfo>;
  closeSession: (sessionId: string) => Promise<void>;
  writeTerminal: (sessionId: string, data: string) => Promise<void>;
  resizeTerminal: (sessionId: string, cols: number, rows: number) => Promise<void>;
  setActiveView: (viewId: string | null) => void;
  applySessionStatus: (payload: SessionStatusEvent) => void;
  applySessionProgress: (payload: SessionProgressEvent) => void;
  initListeners: () => Promise<() => void>;
}

/** 将连接状态映射为用户可读的中文提示。 */
export function statusLabel(status: SessionStatus, error?: AppErrorInfo | null, locale: Locale = useLocaleStore.getState().locale): string {
  switch (status) {
    case SessionStatus.Connecting: return translate(locale, 'session.connectingGeneric');
    case SessionStatus.Connected: return '';
    case SessionStatus.AuthFailed: return error ? formatAppError(locale, error) : translate(locale, 'session.authFailed');
    case SessionStatus.Timeout: return error ? formatAppError(locale, error) : translate(locale, 'session.timeout');
    case SessionStatus.Error: return error ? formatAppError(locale, error) : translate(locale, 'session.error');
    case SessionStatus.Disconnected: return translate(locale, 'session.disconnected');
    default: return translate(locale, 'session.unknown');
  }
}

/** 将连接阶段映射为用户可读的进度文案。 */
export function progressLabel(phase: ConnectionPhase, locale: Locale = useLocaleStore.getState().locale): string {
  return translate(locale, `phase.${phase}` as TranslationKey);
}

/** 将会话状态与连接投影渲染为所属终端区域的用户可读文案；在渲染时按当前语言生成，切换语言后即时生效。 */
export function connectionLabel(
  session: SessionInfo,
  connection: SessionConnection | undefined,
  locale: Locale = useLocaleStore.getState().locale,
): string {
  if (session.status === SessionStatus.Connecting) {
    return connection?.phase
      ? progressLabel(connection.phase, locale)
      : translate(locale, 'session.connecting', { name: `${session.username}@${session.host}` });
  }
  return statusLabel(session.status, connection?.error, locale);
}

/** 判断会话状态是否需要在所属终端区域呈现连接覆盖层（Connecting 或连接失败；Disconnected 保留终端内容不可覆盖）。 */
export function overlayStatus(status: SessionStatus): boolean {
  return status === SessionStatus.Connecting
    || status === SessionStatus.AuthFailed
    || status === SessionStatus.Timeout
    || status === SessionStatus.Error;
}

export const useSessionStore = create<SessionState>((set, get) => {
  return {
    sessions: new Map(),
    activeView: null,
    connections: new Map(),

    /** 打开 SSH 会话，初始化文件传输并启动关联监控任务。 */
    async openSession(hostId) {
      const session = await invoke<SessionInfo>('open_session', { hostId });
      set((state) => ({
        sessions: new Map(state.sessions).set(session.sessionId, session),
        activeView: session.sessionId,
        connections: new Map(state.connections).set(session.sessionId, { phase: null, error: null }),
      }));
      void useSftpStore.getState().listDir(session.sessionId, '/');
      void useSftpStore.getState().loadTaskSnapshot(session.sessionId);
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
        const connections = new Map(state.connections);
        connections.delete(sessionId);
        return { sessions, connections, activeView: state.activeView === sessionId ? null : state.activeView };
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

    /** 应用后端权威会话状态；投影仅更新所属 session，Connected/Disconnected 后清除。 */
    applySessionStatus(payload) {
      const current = get().sessions.get(payload.sessionId);
      // 未知 session（如关闭后迟到的后端事件）无投影可更新，直接丢弃
      if (!current) return;
      const connections = new Map(get().connections);
      if (overlayStatus(payload.status)) {
        connections.set(payload.sessionId, { phase: null, error: payload.error ?? null });
      } else {
        connections.delete(payload.sessionId);
      }
      set((state) => ({
        sessions: new Map(state.sessions).set(payload.sessionId, { ...current, status: payload.status }),
        connections,
      }));
    },

    /** 仅在连接中应用阶段诊断信息，且只写入所属 session。 */
    applySessionProgress(payload) {
      const current = get().sessions.get(payload.sessionId);
      if (current?.status !== SessionStatus.Connecting) return;
      set((state) => ({
        connections: new Map(state.connections).set(payload.sessionId, { phase: payload.phase, error: null }),
      }));
    },

    /** 注册会话状态与连接进度事件，返回统一清理函数。 */
    async initListeners() {
      const unlistenStatus = await listen<SessionStatusEvent>('session:status', (event) => {
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
