import { beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import { invoke } from '@tauri-apps/api/core';
import { emitMockEvent, resetMockEvents } from '@tauri-apps/api/event';
import { useSessionStore } from '@/stores/session';
import { SessionStatus } from '@/types/session';
import { makeSession } from './fixtures';

describe('session store', () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    vi.mocked(invoke).mockReset();
    resetMockEvents();
  });

  it('opens a session and marks it active', async () => {
    vi.mocked(invoke).mockResolvedValueOnce(makeSession());
    const store = useSessionStore();

    const session = await store.openSession('host-1');

    expect(session.session_id).toBe('session-1');
    expect(store.activeSessionId).toBe('session-1');
    expect(store.getOutput('session-1')).toBe('');
    expect(store.statusMessage).toContain('正在连接');
  });

  it('reacts to session and terminal events', async () => {
    vi.mocked(invoke).mockResolvedValueOnce(makeSession());
    const store = useSessionStore();
    const dispose = await store.initListeners();
    await store.openSession('host-1');

    emitMockEvent('session:status', {
      session_id: 'session-1',
      status: SessionStatus.Connected,
      message: null,
    });
    emitMockEvent('terminal:data', {
      session_id: 'session-1',
      data: 'hello',
    });

    expect(store.activeSession?.status).toBe(SessionStatus.Connected);
    expect(store.getOutput('session-1')).toBe('hello');
    expect(store.statusMessage).toBe('已连接');

    dispose();
  });

  it('closes the active session and falls back to the next tab', async () => {
    vi.mocked(invoke)
      .mockResolvedValueOnce(makeSession())
      .mockResolvedValueOnce(makeSession({ session_id: 'session-2', host_id: 'host-2' }))
      .mockResolvedValueOnce(undefined);
    const store = useSessionStore();

    await store.openSession('host-1');
    await store.openSession('host-2');
    await store.closeSession('session-2');

    expect(invoke).toHaveBeenLastCalledWith('close_session', { sessionId: 'session-2' });
    expect(store.activeSessionId).toBe('session-1');
    expect(store.sessionList).toHaveLength(1);
  });
});
