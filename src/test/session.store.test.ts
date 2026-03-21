import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import { invoke } from '@tauri-apps/api/core';
import { emitMockEvent, resetMockEvents } from '@tauri-apps/api/event';
import { useSessionStore } from '@/stores/session';
import { SessionStatus } from '@/types/session';
import { makeSession, makeSnapshot, makeTaskInfo } from './fixtures';

/** 为 openSession 注册所需的两个连续 mock：open_session + start_monitoring */
function mockOpenSession(sessionOverrides = {}, taskOverrides = {}) {
  vi.mocked(invoke)
    .mockResolvedValueOnce(makeSession(sessionOverrides))
    .mockResolvedValueOnce(makeTaskInfo(taskOverrides));
}

describe('session store', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    setActivePinia(createPinia());
    vi.mocked(invoke).mockReset();
    resetMockEvents();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it('opens a session and sets it as active view', async () => {
    mockOpenSession();
    const store = useSessionStore();

    const session = await store.openSession('host-1');

    expect(session.session_id).toBe('session-1');
    expect(store.activeView).toBe('session-1');
    expect(store.statusMessage).toContain('正在连接');
  });

  it('reacts to session:status event and updates session state', async () => {
    mockOpenSession();
    const store = useSessionStore();
    const dispose = await store.initListeners();
    await store.openSession('host-1');

    emitMockEvent('session:status', {
      session_id: 'session-1',
      status: SessionStatus.Connected,
      message: null,
    });

    expect(store.activeSession?.status).toBe(SessionStatus.Connected);
    // Connected 状态清空状态栏消息
    expect(store.statusMessage).toBe('');

    dispose();
  });

  // 需求 8.6：错误类状态应更新为对应中文提示文本
  describe('error state statusMessage mapping (Requirement 8.6)', () => {
    it('sets Chinese message for AuthFailed status', async () => {
      mockOpenSession();
      const store = useSessionStore();
      const dispose = await store.initListeners();
      await store.openSession('host-1');

      emitMockEvent('session:status', {
        session_id: 'session-1',
        status: SessionStatus.AuthFailed,
        message: null,
      });

      expect(store.statusMessage).toBe('认证失败，请检查用户名和密码');
      dispose();
    });

    it('sets Chinese message for Timeout status', async () => {
      mockOpenSession();
      const store = useSessionStore();
      const dispose = await store.initListeners();
      await store.openSession('host-1');

      emitMockEvent('session:status', {
        session_id: 'session-1',
        status: SessionStatus.Timeout,
        message: null,
      });

      expect(store.statusMessage).toBe('连接超时，请检查网络或主机地址');
      dispose();
    });

    it('sets Chinese message for Disconnected status', async () => {
      mockOpenSession();
      const store = useSessionStore();
      const dispose = await store.initListeners();
      await store.openSession('host-1');

      emitMockEvent('session:status', {
        session_id: 'session-1',
        status: SessionStatus.Disconnected,
        message: null,
      });

      expect(store.statusMessage).toBe('连接已断开');
      dispose();
    });

    it('sets Chinese message for Error status without backend message', async () => {
      mockOpenSession();
      const store = useSessionStore();
      const dispose = await store.initListeners();
      await store.openSession('host-1');

      emitMockEvent('session:status', {
        session_id: 'session-1',
        status: SessionStatus.Error,
        message: null,
      });

      expect(store.statusMessage).toBe('连接错误');
      dispose();
    });

    it('includes backend message in Error status text', async () => {
      mockOpenSession();
      const store = useSessionStore();
      const dispose = await store.initListeners();
      await store.openSession('host-1');

      emitMockEvent('session:status', {
        session_id: 'session-1',
        status: SessionStatus.Error,
        message: 'connection refused',
      });

      expect(store.statusMessage).toBe('连接错误：connection refused');
      dispose();
    });

    it('sets Connecting message for Connecting status', async () => {
      mockOpenSession();
      const store = useSessionStore();
      const dispose = await store.initListeners();
      await store.openSession('host-1');

      emitMockEvent('session:status', {
        session_id: 'session-1',
        status: SessionStatus.Connecting,
        message: null,
      });

      expect(store.statusMessage).toBe('正在连接...');
      dispose();
    });
  });

  it('reacts to monitor:snapshot event and stores snapshot', async () => {
    mockOpenSession();
    const store = useSessionStore();
    const dispose = await store.initListeners();
    await store.openSession('host-1');

    emitMockEvent('monitor:snapshot', makeSnapshot({ session_id: 'session-1', cpu_usage: 55.0 }));

    expect(store.snapshots.get('session-1')?.cpu_usage).toBe(55.0);

    dispose();
  });

  it('updates statusMessage from session:progress while session is still connecting', async () => {
    mockOpenSession();
    const store = useSessionStore();
    const dispose = await store.initListeners();
    await store.openSession('host-1');

    emitMockEvent('session:progress', {
      sessionId: 'session-1',
      phase: 'LoadingCredentials',
      message: '正在读取凭据...',
      timestamp: 1_710_000_000_111,
    });

    expect(store.statusMessage).toBe('正在读取凭据...');
    expect(store.activeSession?.status).toBe(SessionStatus.Connecting);

    dispose();
  });

  it('ignores session:progress after a terminal status has been received', async () => {
    mockOpenSession();
    const store = useSessionStore();
    const dispose = await store.initListeners();
    await store.openSession('host-1');

    emitMockEvent('session:status', {
      session_id: 'session-1',
      status: SessionStatus.Timeout,
      message: null,
    });

    emitMockEvent('session:progress', {
      sessionId: 'session-1',
      phase: 'StartingShell',
      message: '正在启动 Shell...',
      timestamp: 1_710_000_000_222,
    });

    expect(store.statusMessage).toBe('连接超时，请检查网络或主机地址');
    expect(store.activeSession?.status).toBe(SessionStatus.Timeout);

    dispose();
  });

  it('marks a session as timeout when connecting exceeds the watchdog threshold', async () => {
    vi.mocked(invoke).mockResolvedValue(undefined);
    mockOpenSession();
    const store = useSessionStore();

    await store.openSession('host-1');
    await vi.advanceTimersByTimeAsync(15_001);

    expect(store.activeSession?.status).toBe(SessionStatus.Timeout);
    expect(store.statusMessage).toBe('连接超时，请检查网络或主机地址');
    expect(invoke).toHaveBeenCalledWith('sync_session_status', {
      sessionId: 'session-1',
      status: SessionStatus.Timeout,
    });
  });

  it('clears the watchdog once a terminal status event arrives', async () => {
    vi.mocked(invoke).mockResolvedValue(undefined);
    mockOpenSession();
    const store = useSessionStore();
    const dispose = await store.initListeners();

    await store.openSession('host-1');
    emitMockEvent('session:status', {
      session_id: 'session-1',
      status: SessionStatus.Connected,
      message: null,
    });

    await vi.advanceTimersByTimeAsync(15_001);

    expect(store.activeSession?.status).toBe(SessionStatus.Connected);

    dispose();
  });

  it('closes the active session and falls back to home view', async () => {
    mockOpenSession({ session_id: 'session-1', host_id: 'host-1' }, { task_id: 'task-1', session_id: 'session-1' });
    mockOpenSession({ session_id: 'session-2', host_id: 'host-2' }, { task_id: 'task-2', session_id: 'session-2' });
    vi.mocked(invoke)
      .mockResolvedValueOnce(undefined)  // stop_monitoring
      .mockResolvedValueOnce(undefined); // close_session
    const store = useSessionStore();

    await store.openSession('host-1');
    await store.openSession('host-2');
    await store.closeSession('session-2');

    expect(invoke).toHaveBeenCalledWith('close_session', { sessionId: 'session-2' });
    expect(store.activeView).toBe('home');
    expect(store.sessionList).toHaveLength(1);
  });

  it('closing a non-active session does not change active view', async () => {
    mockOpenSession({ session_id: 'session-1', host_id: 'host-1' }, { task_id: 'task-1', session_id: 'session-1' });
    mockOpenSession({ session_id: 'session-2', host_id: 'host-2' }, { task_id: 'task-2', session_id: 'session-2' });
    vi.mocked(invoke)
      .mockResolvedValueOnce(undefined)  // stop_monitoring
      .mockResolvedValueOnce(undefined); // close_session
    const store = useSessionStore();

    await store.openSession('host-1');
    await store.openSession('host-2');
    // session-2 is now active; close session-1 instead
    await store.closeSession('session-1');

    expect(store.activeView).toBe('session-2');
    expect(store.sessionList).toHaveLength(1);
  });

  it('setActiveView switches between home and session views', async () => {
    mockOpenSession();
    const store = useSessionStore();
    await store.openSession('host-1');

    store.setActiveView('home');
    expect(store.activeView).toBe('home');
    expect(store.activeSession).toBeNull();

    store.setActiveView('session-1');
    expect(store.activeView).toBe('session-1');
    expect(store.activeSession?.session_id).toBe('session-1');
  });

  // P1-1 修复验证：session:status 事件触发 sync_session_status invoke
  describe('sync_session_status backend sync (P1-1 fix)', () => {
    it('calls sync_session_status when session:status event is received', async () => {
      mockOpenSession();
      const store = useSessionStore();
      const dispose = await store.initListeners();
      await store.openSession('host-1');

      vi.mocked(invoke).mockResolvedValueOnce(undefined); // sync_session_status
      emitMockEvent('session:status', {
        session_id: 'session-1',
        status: SessionStatus.Connected,
        message: null,
      });

      await Promise.resolve();

      expect(invoke).toHaveBeenCalledWith('sync_session_status', {
        sessionId: 'session-1',
        status: SessionStatus.Connected,
      });

      dispose();
    });

    it('sync_session_status is called for each distinct status transition', async () => {
      // mockResolvedValue (no Once) acts as fallback for all remaining calls
      vi.mocked(invoke).mockResolvedValue(undefined);
      mockOpenSession();
      const store = useSessionStore();
      const dispose = await store.initListeners();
      await store.openSession('host-1');

      const statuses = [SessionStatus.Connected, SessionStatus.Disconnected];
      for (const status of statuses) {
        emitMockEvent('session:status', { session_id: 'session-1', status, message: null });
        await Promise.resolve();
        expect(invoke).toHaveBeenCalledWith('sync_session_status', {
          sessionId: 'session-1',
          status,
        });
      }

      dispose();
    });
  });
});
