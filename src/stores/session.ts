import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { create } from 'zustand';
import type { SessionInfo, SessionProgressEvent, SessionStatusEvent } from '@/types/session';
import { ConnectionPhase, SessionStatus } from '@/types/session';
import type { AppErrorInfo, Locale, TranslationKey } from '@/i18n';
import { formatAppError, translate } from '@/i18n';
import { useLocaleStore } from '@/stores/locale';
import { useMonitorStore } from './monitor';
import { useSftpStore } from './sftp';

interface SessionState {
  sessions: Map<string, SessionInfo>;
  activeView: string | null;
  statusMessage: string;
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

/** 将连接阶段映射为用户可读的中文进度。 */
export function progressLabel(phase: ConnectionPhase, locale: Locale = useLocaleStore.getState().locale): string {
  return translate(locale, `phase.${phase}` as TranslationKey);
}

export const useSessionStore = create<SessionState>((set, get) => {
  return {
    sessions: new Map(),
    activeView: null,
    statusMessage: translate(useLocaleStore.getState().locale, 'session.ready'),

    /** 打开 SSH 会话，并启动关联监控任务。 */
    async openSession(hostId) {
      const session = await invoke<SessionInfo>('open_session', { hostId });
      set((state) => ({
        sessions: new Map(state.sessions).set(session.sessionId, session),
        activeView: session.sessionId,
        statusMessage: translate(useLocaleStore.getState().locale, 'session.connecting', { name: `${session.username}@${session.host}` }),
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
        statusMessage: statusLabel(payload.status, payload.error),
      }));
      if (current && payload.status === SessionStatus.Connected) {
        useSftpStore.getState().listDir(payload.sessionId, '/').catch(() => {});
      }
    },

    /** 仅在连接中应用阶段诊断信息。 */
    applySessionProgress(payload) {
      const current = get().sessions.get(payload.sessionId);
      if (current?.status === SessionStatus.Connecting) {
        set({ statusMessage: progressLabel(payload.phase) });
      }
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
