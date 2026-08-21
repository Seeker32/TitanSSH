import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { create } from 'zustand';
import type { HostIdentityChallenge, HostIdentityChallengeDismissed, SessionConnection, SessionInfo, SessionProgressEvent, SessionStatusEvent } from '@/types/session';
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
  /** 投影建立前到达的进度事件缓存（open_session 返回与首个 progress 事件的 IPC 竞态）；
   *  openSession 建立投影时回放并清除，removeSessionProjection 一并丢弃。 */
  pendingProgress: Map<string, SessionProgressEvent>;
  /** 按 sessionId 存储的主机身份确认投影；接受/拒绝后清除。 */
  hostKeyChallenges: Map<string, HostIdentityChallenge>;
  /** 按 sessionId 存储的"接受并保存"结构化失败；challenge 保持未决，改选或新 challenge 后清除。 */
  hostKeySaveErrors: Map<string, AppErrorInfo>;
  openSession: (hostId: string) => Promise<SessionInfo>;
  closeSession: (sessionId: string) => Promise<void>;
  writeTerminal: (sessionId: string, data: string) => Promise<void>;
  resizeTerminal: (sessionId: string, cols: number, rows: number) => Promise<void>;
  setActiveView: (viewId: string | null) => void;
  applySessionStatus: (payload: SessionStatusEvent) => void;
  applySessionProgress: (payload: SessionProgressEvent) => void;
  applyHostIdentityChallenge: (payload: HostIdentityChallenge) => void;
  applyHostIdentityChallengeDismissed: (payload: HostIdentityChallengeDismissed) => void;
  acceptAndSaveHostIdentity: (sessionId: string) => Promise<void>;
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

/** 从不可变投影副本中移除指定 Session 的主机身份确认卡与保存错误。 */
function withoutHostKeyProjection(
  hostKeyChallenges: Map<string, HostIdentityChallenge>,
  hostKeySaveErrors: Map<string, AppErrorInfo>,
  sessionId: string,
): { hostKeyChallenges: Map<string, HostIdentityChallenge>; hostKeySaveErrors: Map<string, AppErrorInfo> } {
  const challenges = new Map(hostKeyChallenges);
  challenges.delete(sessionId);
  const saveErrors = new Map(hostKeySaveErrors);
  saveErrors.delete(sessionId);
  return { hostKeyChallenges: challenges, hostKeySaveErrors: saveErrors };
}

export const useSessionStore = create<SessionState>((set, get) => {
  return {
    sessions: new Map(),
    activeView: null,
    connections: new Map(),
    pendingProgress: new Map(),
    hostKeyChallenges: new Map(),
    hostKeySaveErrors: new Map(),

    /** 从前端投影移除会话及其关联状态；后端 teardown 由调用方保证。 */
    removeSessionProjection(sessionId: string) {
      set((state) => {
        const sessions = new Map(state.sessions);
        sessions.delete(sessionId);
        const connections = new Map(state.connections);
        connections.delete(sessionId);
        const pendingProgress = new Map(state.pendingProgress);
        pendingProgress.delete(sessionId);
        const projection = withoutHostKeyProjection(state.hostKeyChallenges, state.hostKeySaveErrors, sessionId);
        return { sessions, connections, pendingProgress, ...projection, activeView: state.activeView === sessionId ? null : state.activeView };
      });
      useMonitorStore.getState().clearSession(sessionId);
      useSftpStore.getState().clearSession(sessionId);
    },

    /** 打开 SSH 会话，初始化文件传输并启动关联监控任务。 */
    async openSession(hostId) {
      const session = await invoke<SessionInfo>('open_session', { hostId });
      set((state) => {
        // 回放竞态期间缓存的进度事件：后端 worker 在 open_session 返回前
        // 就可能已发出首个阶段事件，此时阶段不得丢回 null
        const pendingPhase = state.pendingProgress.get(session.sessionId)?.phase ?? null;
        const pendingProgress = new Map(state.pendingProgress);
        pendingProgress.delete(session.sessionId);
        return {
          sessions: new Map(state.sessions).set(session.sessionId, session),
          activeView: session.sessionId,
          connections: new Map(state.connections).set(session.sessionId, { phase: pendingPhase, error: null }),
          pendingProgress,
        };
      });
      useSftpStore.getState().ensureState(session.sessionId);
      void useSftpStore.getState().listDir(session.sessionId, '/');
      void useSftpStore.getState().loadTaskSnapshot(session.sessionId);
      try {
        await useMonitorStore.getState().startMonitoring(session.sessionId);
      } catch {
        // 监控失败不阻断 SSH 主流程。
      }
      return session;
    },

    /** 关闭 SSH 会话：请求后端统一 teardown，无论后端结果如何都移除前端 projection。
     *
     * 连接失败（如凭据不存在）或断连后，终端工作线程退出时 TerminalExitGuard 已完成后端
     * teardown，close_session 会返回 SessionNotFound —— 标签是纯视图，关闭视图是本地操作，
     * 不得被后端会话状态阻塞；其他后端错误同样只记录诊断，不阻塞投影清理。
     */
    async closeSession(sessionId) {
      useSftpStore.getState().markSessionClosing(sessionId);
      try {
        await invoke('close_session', { sessionId });
      } catch (error) {
        console.warn('[session] close_session rejected, removing projection anyway:', sessionId, error);
      } finally {
        get().removeSessionProjection(sessionId);
        useSftpStore.getState().finishSessionClosing(sessionId);
      }
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
      // 仅 Connected 证明认证已越过验证门：清理确认卡与保存错误投影，覆盖跨
      // Session 保存自动放行其他标签的路径。其余状态（Error/Timeout/AuthFailed）
      // 可能携带错误覆盖层，未决 challenge 必须保持可见可决，不做隐式清理。
      let projection = { hostKeyChallenges: get().hostKeyChallenges, hostKeySaveErrors: get().hostKeySaveErrors };
      if (payload.status === SessionStatus.Connected) {
        projection = withoutHostKeyProjection(projection.hostKeyChallenges, projection.hostKeySaveErrors, payload.sessionId);
      }
      set((state) => ({
        sessions: new Map(state.sessions).set(payload.sessionId, { ...current, status: payload.status }),
        connections,
        ...projection,
      }));
    },

    /** 仅在连接中应用阶段诊断信息；投影未建立时缓存待回放，而不是丢弃。 */
    applySessionProgress(payload) {
      const current = get().sessions.get(payload.sessionId);
      if (!current) {
        // open_session 返回前 worker 已发出首个进度事件（IPC 竞态）：
        // 缓存最新一条，投影建立时回放，避免标签永远显示通用“正在连接”
        set((state) => ({
          pendingProgress: new Map(state.pendingProgress).set(payload.sessionId, payload),
        }));
        return;
      }
      if (current.status !== SessionStatus.Connecting) return;
      set((state) => ({
        connections: new Map(state.connections).set(payload.sessionId, { phase: payload.phase, error: null }),
      }));
    },

    /** 应用主机身份确认事件：按 sessionId 存储确认卡投影；新 challenge 清除此前的保存错误。 */
    applyHostIdentityChallenge(payload) {
      set((state) => {
        const hostKeySaveErrors = new Map(state.hostKeySaveErrors);
        hostKeySaveErrors.delete(payload.sessionId);
        return {
          hostKeyChallenges: new Map(state.hostKeyChallenges).set(payload.sessionId, payload),
          hostKeySaveErrors,
        };
      });
    },

    /** 应用后端挑战撤销事件：仅当当前确认卡仍是同一 challenge 时撤下（含保存错误），
     *  不得误删已被新 challenge 取代的投影（旧撤销迟到于新 challenge 事件时）。 */
    applyHostIdentityChallengeDismissed(payload) {
      set((state) => {
        const current = state.hostKeyChallenges.get(payload.sessionId);
        if (!current || current.challengeId !== payload.challengeId) return state;
        return withoutHostKeyProjection(state.hostKeyChallenges, state.hostKeySaveErrors, payload.sessionId);
      });
    },

    /** 接受并保存：把 challenge 快照的公钥持久化为长期信任并放行当前 Session。
     *  保存失败（HostKeySaveFailed）时 challenge 保持未决，结构化错误显示在所属标签，
     *  绝不自动降级为临时信任；用户可重试保存、改选仅本次接受或拒绝。 */
    async acceptAndSaveHostIdentity(sessionId) {
      const challenge = get().hostKeyChallenges.get(sessionId);
      if (!challenge) return;
      try {
        await invoke('accept_and_save_host_identity', { challengeId: challenge.challengeId });
      } catch (error) {
        const appError = toAppError(error);
        if (appError.code !== 'HostKeyChallengeNotFound') {
          set((state) => ({
            hostKeySaveErrors: new Map(state.hostKeySaveErrors).set(sessionId, appError),
          }));
          return;
        }
        // challenge 已不存在（并发解决）：仅撤下过期确认卡
      }
      set((state) => ({
        ...withoutHostKeyProjection(state.hostKeyChallenges, state.hostKeySaveErrors, sessionId),
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
      set((state) => ({
        ...withoutHostKeyProjection(state.hostKeyChallenges, state.hostKeySaveErrors, sessionId),
      }));
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
        set((state) => ({
          ...withoutHostKeyProjection(state.hostKeyChallenges, state.hostKeySaveErrors, sessionId),
        }));
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
      const unlistenDismissed = await listen<HostIdentityChallengeDismissed>('host-identity:challenge-dismissed', (event) => {
        get().applyHostIdentityChallengeDismissed(event.payload);
      });
      return () => {
        unlistenStatus();
        unlistenProgress();
        unlistenChallenge();
        unlistenDismissed();
      };
    },
  };
});
