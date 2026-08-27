import { describe, expect, it } from 'vitest';
import { SessionStatus } from '@/types/session';
import {
  emptyTabViewState,
  projectTabViews,
  transitionTabViews,
  type TabViewState,
} from '@/stores/tab-view';
import { makeSession } from './fixtures';

function sessions(...values: ReturnType<typeof makeSession>[]) {
  return new Map(values.map((session) => [session.sessionId, session]));
}

function stateWithSession(sessionId = 'session-1'): TabViewState {
  return transitionTabViews(emptyTabViewState, sessions(makeSession({ sessionId })), {
    type: 'session-opened', sessionId, createdAt: 10,
  }).state;
}

describe('tab-view module', () => {
  it('creates deterministic terminal anchors and process views in insertion order', () => {
    const session = makeSession();
    const sessionMap = sessions(session);
    const opened = transitionTabViews(emptyTabViewState, sessionMap, {
      type: 'session-opened', sessionId: session.sessionId, createdAt: 10,
    });
    const process = transitionTabViews(opened.state, sessionMap, {
      type: 'process-opened', sessionId: session.sessionId, createdAt: 20,
    });

    expect([...process.state.byId.keys()]).toEqual(['terminal:session-1', 'process:session-1']);
    expect(process.state.activeId).toBe('process:session-1');
    expect(transitionTabViews(process.state, sessionMap, {
      type: 'process-opened', sessionId: session.sessionId, createdAt: 30,
    }).state).toEqual(process.state);
  });

  it('rejects stale actions and unknown sessions', () => {
    expect(transitionTabViews(emptyTabViewState, new Map(), {
      type: 'process-opened', sessionId: 'ghost', createdAt: 1,
    }).state).toEqual(emptyTabViewState);
    const state = stateWithSession();
    expect(transitionTabViews(state, sessions(makeSession()), {
      type: 'activated', tabId: 'terminal:ghost',
    }).state).toBe(state);
    expect(transitionTabViews(state, sessions(makeSession()), {
      type: 'close-requested', tabId: 'terminal:ghost',
    }).state).toBe(state);
  });

  it('emits one anchor close effect and removes the session with right-side fallback', () => {
    const first = makeSession();
    const second = makeSession({ sessionId: 'session-2' });
    const sessionMap = sessions(first, second);
    let state = transitionTabViews(emptyTabViewState, sessionMap, {
      type: 'session-opened', sessionId: first.sessionId, createdAt: 1,
    }).state;
    state = transitionTabViews(state, sessionMap, {
      type: 'session-opened', sessionId: second.sessionId, createdAt: 2,
    }).state;
    const closing = transitionTabViews(state, sessionMap, {
      type: 'close-requested', tabId: 'terminal:session-1',
    });
    expect(closing.effect).toEqual({ type: 'close-session', sessionId: 'session-1' });
    expect(transitionTabViews(closing.state, sessionMap, {
      type: 'close-requested', tabId: 'terminal:session-1',
    }).effect).toBeNull();
    expect(transitionTabViews(closing.state, sessionMap, {
      type: 'session-removed', sessionId: 'session-1',
    }).state.activeId).toBe('terminal:session-2');
  });

  it('closes process views back to their anchor and projects labels, tones and content', () => {
    const session = makeSession({ status: SessionStatus.Connected });
    const map = sessions(session);
    let state = stateWithSession();
    state = transitionTabViews(state, map, { type: 'process-opened', sessionId: 'session-1', createdAt: 2 }).state;
    const closed = transitionTabViews(state, map, { type: 'close-requested', tabId: 'process:session-1' }).state;
    expect(closed.activeId).toBe('terminal:session-1');
    const projection = projectTabViews(state, map, 'zh-CN');
    expect(projection.strip.map((item) => item.label)).toEqual(['root@10.0.0.8', '进程 · root@10.0.0.8']);
    expect(projection.strip[0].statusTone).toBe('connected');
    expect(projection.terminals).toEqual([{ tabId: 'terminal:session-1', session, active: false }]);
    expect(projection.active).toEqual({ kind: 'process', tabId: 'process:session-1', sessionId: 'session-1' });
    expect(projection.activeSessionId).toBe('session-1');
  });

  it('maps every session status and falls back right, then left, then empty', () => {
    const statuses = [
      [SessionStatus.Connecting, 'connecting'],
      [SessionStatus.Connected, 'connected'],
      [SessionStatus.AuthFailed, 'error'],
      [SessionStatus.Timeout, 'error'],
      [SessionStatus.Error, 'error'],
      [SessionStatus.Disconnected, 'error'],
    ] as const;
    for (const [status, tone] of statuses) {
      const session = makeSession({ status });
      expect(projectTabViews(stateWithSession(), sessions(session), 'zh-CN').strip[0].statusTone).toBe(tone);
    }

    const first = makeSession();
    const second = makeSession({ sessionId: 'session-2' });
    const third = makeSession({ sessionId: 'session-3' });
    const map = sessions(first, second, third);
    let state = transitionTabViews(emptyTabViewState, map, { type: 'session-opened', sessionId: first.sessionId, createdAt: 1 }).state;
    state = transitionTabViews(state, map, { type: 'session-opened', sessionId: second.sessionId, createdAt: 2 }).state;
    state = transitionTabViews(state, map, { type: 'session-opened', sessionId: third.sessionId, createdAt: 3 }).state;
    state = transitionTabViews(state, map, { type: 'session-removed', sessionId: 'session-2' }).state;
    expect(state.activeId).toBe('terminal:session-3');
    state = transitionTabViews(state, map, { type: 'session-removed', sessionId: 'session-3' }).state;
    expect(state.activeId).toBe('terminal:session-1');
    expect(transitionTabViews(state, map, { type: 'session-removed', sessionId: 'session-1' }).state.activeId).toBeNull();
  });

  it('skips orphaned views and returns empty content', () => {
    const state = stateWithSession();
    const orphaned = { ...state, byId: new Map([...state.byId, ['process:ghost', {
      id: 'process:ghost', kind: 'process' as const, sessionId: 'ghost', createdAt: 2,
    }]]) };
    const projection = projectTabViews(orphaned, new Map(), 'zh-CN');
    expect(projection.strip).toEqual([]);
    expect(projection.terminals).toEqual([]);
    expect(projection.active).toEqual({ kind: 'empty' });
    expect(projection.activeSessionId).toBeNull();
  });
});
