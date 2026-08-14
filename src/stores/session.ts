import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { create } from 'zustand';
import type { HostIdentityChallenge, SessionConnection, SessionInfo, SessionProgressEvent, SessionStatusEvent } from '@/types/session';
import { ConnectionPhase, SessionStatus } from '@/types/session';
import type { AppErrorInfo, Locale, TranslationKey } from '@/i18n';
import { formatAppError, toAppError, translate } from '@/i18n';
import { useLocaleStore } from '@/stores/locale';
import { useMonitorStore } from './monitor';
import { useSftpStore } from './sftp';

interface SessionState {
  sessions: Map<string, SessionInfo>;
  activeView: string | null;
  /** 按 sessionId 存储的连接生命周期投影（阶段 + 结构化错误）；Connected/Disconnected 后清除。 */
  connections: Map<string, SessionConnection>;
  /** 按 sessionId 存储的主机身份确认投影；接受/拒绝后清除。 */
  hostKeyChallenges: Map<string, HostIdentityChallenge>;
  openSession: (hostId: string) => Promise<SessionInfo>;
  closeSession: (sessionId: string) => Promise<void>;
  writeTerminal: (sessionId: string, data: string) => Promise<void>;
  resizeTerminal: (sessionId: string, cols: number, rows: number) => Promise<void>;
  setActiveView: (viewId: string | null) => void;
  applySessionStatus: (payload: SessionStatusEvent) => void;
  applySessionProgress: (payload: SessionProgressEvent) => void;
  applyHostIdentityChallenge: (payload: HostIdentityChallenge) => void;
  acceptHostIdentity: (sessionId: string) => Promise<void>;
  rejectHostIdentity: (sessionId: string) => Promise<void>;
  removeSessionProjection: (sessionId: string) => void;
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
    hostKeyChallenges: new Map(),

    /** 从前端投影移除会话及其关联状态；后端 teardown 由调用方保证。 */
    removeSessionProjection(sessionId: string) {
      set((state) => {
        const sessions = new Map(state.sessions);
        sessions.delete(sessionId);
        const connections = new Map(state.connections);
        connections.delete(sessionId);
        const hostKeyChallenges = new Map(state.hostKeyChallenges);
        hostKeyChallenges.delete(sessionId);
        return { sessions, connections, hostKeyChallenges, activeView: state.activeView === sessionId ? null : state.activeView };
      });
      useMonitorStore.getState().clearSession(sessionId);
      useSftpStore.getState().clearSession(sessionId);
    },

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
      get().removeSessionProjection(sessionId);
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

    /** 应用主机身份确认事件：按 sessionId 存储确认卡投影。 */
    applyHostIdentityChallenge(payload) {
      set((state) => ({
        hostKeyChallenges: new Map(state.hostKeyChallenges).set(payload.sessionId, payload),
      }));
    },

    /** 仅本次接受未知主机身份：接受该 Runtime Session 的 Terminal、SFTP、Monitoring 及重连。
     *  challenge 已不存在（重复操作/并发解决）时仅撤下过期确认卡，不误杀会话投影。 */
    async acceptHostIdentity(sessionId) {
      const challenge = get().hostKeyChallenges.get(sessionId);
      if (!challenge) return;
      try {
        await invoke('accept_host_identity', { challengeId: challenge.challengeId });
      } catch (error) {
        // 后端权威：challenge 已解决时确认卡投影已过期；其他错误保留确认卡，避免掩盖未决决定
        if (toAppError(error).code !== 'HostKeyChallengeNotFound') return;
      }
      set((state) => {
        const hostKeyChallenges = new Map(state.hostKeyChallenges);
        hostKeyChallenges.delete(sessionId);
        return { hostKeyChallenges };
      });
    },

    /** 拒绝未知主机身份并关闭整个 Session：Terminal、SFTP 与 Monitoring 服从同一决定。
     *  后端在拒绝命令内完成 teardown，前端只清理本地投影，不重复 close_session。
     *  challenge 已不存在时仅撤下过期确认卡，不误杀仍存活的会话投影。 */
    async rejectHostIdentity(sessionId) {
      const challenge = get().hostKeyChallenges.get(sessionId);
      if (!challenge) return;
      try {
        await invoke('reject_host_identity', { challengeId: challenge.challengeId });
      } catch (error) {
        if (toAppError(error).code !== 'HostKeyChallengeNotFound') return;
        // 决定已生效（重复操作/并发）：仅撤下确认卡，会话可能仍存活
        set((state) => {
          const hostKeyChallenges = new Map(state.hostKeyChallenges);
          hostKeyChallenges.delete(sessionId);
          return { hostKeyChallenges };
        });
        return;
      }
      get().removeSessionProjection(sessionId);
    },

    /** 注册会话状态、连接进度与主机身份确认事件，返回统一清理函数。 */
    async initListeners() {
      const unlistenStatus = await listen<SessionStatusEvent>('session:status', (event) => {
        get().applySessionStatus(event.payload);
      });
      const unlistenProgress = await listen<SessionProgressEvent>('session:progress', (event) => {
        get().applySessionProgress(event.payload);
      });
      const unlistenChallenge = await listen<HostIdentityChallenge>('host-identity:challenge', (event) => {
        get().applyHostIdentityChallenge(event.payload);
      });
      return () => {
        unlistenStatus();
        unlistenProgress();
        unlistenChallenge();
      };
    },
  };
});
