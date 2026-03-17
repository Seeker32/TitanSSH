import { beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import { invoke } from '@tauri-apps/api/core';
import { emitMockEvent, resetMockEvents } from '@tauri-apps/api/event';
import { useMonitorStore } from '@/stores/monitor';
import { useSessionStore } from '@/stores/session';
import { makeSession, makeSnapshot } from './fixtures';

describe('monitor store', () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    vi.mocked(invoke).mockReset();
    resetMockEvents();
  });

  it('fetches and stores a monitor snapshot', async () => {
    vi.mocked(invoke).mockResolvedValueOnce(makeSnapshot());
    const store = useMonitorStore();

    const snapshot = await store.fetchSnapshot('session-1');

    expect(invoke).toHaveBeenCalledWith('get_monitor_status', { sessionId: 'session-1' });
    expect(snapshot.cpu_usage).toBe(21.5);
    expect(store.snapshots.get('session-1')?.memory_usage).toBe(25.0);
  });

  it('updates active snapshot when monitor:snapshot event arrives', async () => {
    const sessionStore = useSessionStore();
    vi.mocked(invoke).mockResolvedValueOnce(makeSession());
    await sessionStore.openSession('host-1');

    const store = useMonitorStore();
    const dispose = await store.initListeners();

    emitMockEvent('monitor:snapshot', makeSnapshot({ session_id: 'session-1', cpu_usage: 77.3 }));

    expect(store.activeSnapshot?.cpu_usage).toBe(77.3);
    dispose();
  });

  it('activeSnapshot returns null when home view is active', () => {
    const sessionStore = useSessionStore();
    sessionStore.setActiveView('home');

    const store = useMonitorStore();
    expect(store.activeSnapshot).toBeNull();
  });

  it('snapshot timestamp is in milliseconds', async () => {
    vi.mocked(invoke).mockResolvedValueOnce(makeSnapshot());
    const store = useMonitorStore();

    const snapshot = await store.fetchSnapshot('session-1');

    // 毫秒时间戳应大于 1_000_000_000_000
    expect(snapshot.timestamp).toBeGreaterThan(1_000_000_000_000);
  });
});
