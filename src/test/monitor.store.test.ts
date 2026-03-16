import { beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import { invoke } from '@tauri-apps/api/core';
import { emitMockEvent, resetMockEvents } from '@tauri-apps/api/event';
import { useMonitorStore } from '@/stores/monitor';
import { useSessionStore } from '@/stores/session';
import { makeSession, makeStatus } from './fixtures';

describe('monitor store', () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    vi.mocked(invoke).mockReset();
    resetMockEvents();
  });

  it('fetches and stores a server status snapshot', async () => {
    vi.mocked(invoke).mockResolvedValueOnce(makeStatus());
    const store = useMonitorStore();

    const status = await store.fetchStatus('session-1');

    expect(invoke).toHaveBeenCalledWith('get_server_status', { sessionId: 'session-1' });
    expect(status.ip).toBe('10.0.0.8');
    expect(store.statuses.get('session-1')?.cpu_percent).toBe(21.5);
  });

  it('updates active status when monitor event arrives', async () => {
    const sessionStore = useSessionStore();
    sessionStore.sessions = new Map([['session-1', makeSession()]]);
    sessionStore.activeSessionId = 'session-1';

    const store = useMonitorStore();
    const dispose = await store.initListeners();
    emitMockEvent('monitor:update', makeStatus({ session_id: 'session-1', ip: '172.16.0.3' }));

    expect(store.activeStatus?.ip).toBe('172.16.0.3');
    dispose();
  });
});
