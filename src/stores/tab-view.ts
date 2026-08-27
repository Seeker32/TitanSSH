import type { Locale } from '@/i18n';
import { translate } from '@/i18n';
import { SessionStatus, type SessionInfo } from '@/types/session';

export type TabKind = 'terminal' | 'process';
export type TabId = `${TabKind}:${string}`;
export type TabStatusTone = 'connected' | 'connecting' | 'error' | 'offline';

export type TabView =
  | Readonly<{ id: `terminal:${string}`; kind: 'terminal'; sessionId: string; createdAt: number }>
  | Readonly<{ id: `process:${string}`; kind: 'process'; sessionId: string; createdAt: number }>;

export interface TabViewState {
  readonly byId: ReadonlyMap<TabId, TabView>;
  readonly activeId: TabId | null;
  /** 正在执行 anchor close effect 的 Session；阻止重复 close_session。 */
  readonly closingSessionIds: ReadonlySet<string>;
}

export type TabViewAction =
  | { type: 'session-opened'; sessionId: string; createdAt: number }
  | { type: 'process-opened'; sessionId: string; createdAt: number }
  | { type: 'activated'; tabId: TabId }
  | { type: 'close-requested'; tabId: TabId }
  | { type: 'session-removed'; sessionId: string };

export type TabViewEffect = { type: 'close-session'; sessionId: string };

export interface TabViewTransition {
  readonly state: TabViewState;
  readonly effect: TabViewEffect | null;
}

export interface TabStripItem {
  readonly id: TabId;
  readonly label: string;
  readonly statusTone: TabStatusTone;
  readonly active: boolean;
}

export interface TerminalViewport {
  readonly tabId: TabId;
  readonly session: SessionInfo;
  readonly active: boolean;
}

export type ActiveTabView =
  | { readonly kind: 'empty' }
  | { readonly kind: 'terminal'; readonly tabId: TabId; readonly sessionId: string }
  | { readonly kind: 'process'; readonly tabId: TabId; readonly sessionId: string };

export interface TabViewProjection {
  readonly strip: readonly TabStripItem[];
  readonly terminals: readonly TerminalViewport[];
  readonly active: ActiveTabView;
  readonly activeSessionId: string | null;
}

export const emptyTabViewState: TabViewState = {
  byId: new Map(),
  activeId: null,
  closingSessionIds: new Set(),
};

/** 生成确定性标签 ID；Session store 不需要知道标签命名规则。 */
function idOf(kind: TabKind, sessionId: string): TabId {
  return `${kind}:${sessionId}`;
}

/** 复制标签状态并替换指定字段，保持无变化动作的引用稳定。 */
function withTabState(byId: ReadonlyMap<TabId, TabView>, activeId: TabId | null, closingSessionIds: ReadonlySet<string>): TabViewState {
  return { byId, activeId, closingSessionIds };
}

/** 应用一次标签动作，集中维护创建、激活、关闭和 Session 回退不变量。 */
export function transitionTabViews(
  state: TabViewState,
  sessions: ReadonlyMap<string, SessionInfo>,
  action: TabViewAction,
): TabViewTransition {
  if (action.type === 'session-opened') {
    if (!sessions.has(action.sessionId)) return { state, effect: null };
    const id = idOf('terminal', action.sessionId) as `terminal:${string}`;
    const existing = state.byId.get(id);
    if (existing) return { state: existing.kind === 'terminal' && state.activeId === id ? state : withTabState(state.byId, id, state.closingSessionIds), effect: null };
    const byId = new Map(state.byId);
    byId.set(id, { id, kind: 'terminal', sessionId: action.sessionId, createdAt: action.createdAt });
    return { state: withTabState(byId, id, state.closingSessionIds), effect: null };
  }

  if (action.type === 'process-opened') {
    const terminalId = idOf('terminal', action.sessionId) as `terminal:${string}`;
    if (!sessions.has(action.sessionId) || !state.byId.has(terminalId)) return { state, effect: null };
    const id = idOf('process', action.sessionId) as `process:${string}`;
    if (state.byId.has(id)) return { state: state.activeId === id ? state : withTabState(state.byId, id, state.closingSessionIds), effect: null };
    const byId = new Map(state.byId);
    byId.set(id, { id, kind: 'process', sessionId: action.sessionId, createdAt: action.createdAt });
    return { state: withTabState(byId, id, state.closingSessionIds), effect: null };
  }

  if (action.type === 'activated') {
    const tab = state.byId.get(action.tabId);
    if (!tab || !sessions.has(tab.sessionId) || state.activeId === action.tabId) return { state, effect: null };
    return { state: withTabState(state.byId, action.tabId, state.closingSessionIds), effect: null };
  }

  if (action.type === 'close-requested') {
    const tab = state.byId.get(action.tabId);
    if (!tab) return { state, effect: null };
    if (tab.kind === 'terminal') {
      if (state.closingSessionIds.has(tab.sessionId)) return { state, effect: null };
      const closingSessionIds = new Set(state.closingSessionIds);
      closingSessionIds.add(tab.sessionId);
      return { state: withTabState(state.byId, state.activeId, closingSessionIds), effect: { type: 'close-session', sessionId: tab.sessionId } };
    }
    const byId = new Map(state.byId);
    byId.delete(action.tabId);
    const terminalId = idOf('terminal', tab.sessionId) as `terminal:${string}`;
    const activeId = state.activeId === action.tabId ? (byId.has(terminalId) ? terminalId : null) : state.activeId;
    return { state: withTabState(byId, activeId, state.closingSessionIds), effect: null };
  }

  const entries = [...state.byId.entries()];
  const removed = new Set(entries.filter(([, tab]) => tab.sessionId === action.sessionId).map(([id]) => id));
  if (removed.size === 0 && !state.closingSessionIds.has(action.sessionId)) return { state, effect: null };
  const byId = new Map(state.byId);
  removed.forEach((id) => byId.delete(id));
  let activeId = state.activeId;
  if (activeId !== null && removed.has(activeId)) {
    const activeIndex = entries.findIndex(([id]) => id === activeId);
    activeId = entries.slice(activeIndex + 1).find(([id]) => !removed.has(id))?.[0] ??
      [...entries.slice(0, activeIndex)].reverse().find(([id]) => !removed.has(id))?.[0] ?? null;
  }
  const closingSessionIds = new Set(state.closingSessionIds);
  closingSessionIds.delete(action.sessionId);
  return { state: withTabState(byId, activeId, closingSessionIds), effect: null };
}

/** 将 Session 状态映射为标签栏使用的状态色。 */
function statusTone(status: SessionStatus): TabStatusTone {
  if (status === SessionStatus.Connected) return 'connected';
  if (status === SessionStatus.Connecting) return 'connecting';
  if ([SessionStatus.AuthFailed, SessionStatus.Error, SessionStatus.Timeout, SessionStatus.Disconnected].includes(status)) return 'error';
  return 'offline';
}

/** 生成标签栏、常驻终端和活动内容的只读投影，不产生副作用。 */
export function projectTabViews(
  state: TabViewState,
  sessions: ReadonlyMap<string, SessionInfo>,
  locale: Locale,
): TabViewProjection {
  const strip: TabStripItem[] = [];
  const terminals: TerminalViewport[] = [];
  let active: ActiveTabView = { kind: 'empty' };
  for (const tab of state.byId.values()) {
    const session = sessions.get(tab.sessionId);
    if (!session) continue;
    const activeTab = state.activeId === tab.id;
    const name = `${session.username}@${session.host}`;
    strip.push({ id: tab.id, label: tab.kind === 'process' ? `${translate(locale, 'process.tab')} · ${name}` : name, statusTone: statusTone(session.status), active: activeTab });
    if (tab.kind === 'terminal') terminals.push({ tabId: tab.id, session, active: activeTab });
    if (activeTab) active = { kind: tab.kind, tabId: tab.id, sessionId: tab.sessionId };
  }
  return { strip, terminals, active, activeSessionId: active.kind === 'empty' ? null : active.sessionId };
}
