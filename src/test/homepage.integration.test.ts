import { beforeEach, describe, expect, it, vi } from 'vitest';
import { flushPromises, mount } from '@vue/test-utils';
import { createPinia } from 'pinia';
import { nextTick } from 'vue';
import HomePage from '@/pages/HomePage.vue';
import { invoke } from '@tauri-apps/api/core';
import { emitMockEvent, resetMockEvents } from '@tauri-apps/api/event';
import { makeHost, makeSession, makeSnapshot, makeTaskInfo } from './fixtures';

vi.mock('@xterm/xterm', () => ({
  Terminal: class {
    cols = 120;
    rows = 32;
    loadAddon() {}
    open() {}
    write() {}
    clear() {}
    onData() { return { dispose() {} }; }
    dispose() {}
  },
}));

vi.mock('@xterm/addon-fit', () => ({
  FitAddon: class {
    fit() {}
  },
}));

class ResizeObserverMock {
  observe() {}
  disconnect() {}
}

async function flushUi() {
  await flushPromises();
  await Promise.resolve();
  await nextTick();
  await Promise.resolve();
  await nextTick();
}

describe('HomePage integration', () => {
  beforeEach(() => {
    vi.mocked(invoke).mockReset();
    resetMockEvents();
    vi.stubGlobal('ResizeObserver', ResizeObserverMock);
  });

  it('loads hosts and opens a session from the host list', async () => {
    vi.mocked(invoke)
      .mockResolvedValueOnce([makeHost()])   // list_hosts
      .mockResolvedValueOnce(makeSession())  // open_session
      .mockResolvedValueOnce(makeTaskInfo()); // start_monitoring

    const wrapper = mount(HomePage, {
      global: {
        plugins: [createPinia()],
        stubs: {
          Teleport: true,
        },
      },
    });

    await flushUi();

    expect(invoke).toHaveBeenCalledWith('list_hosts');
    expect(wrapper.text()).toContain('prod');

    await wrapper.get('.host-card').trigger('click');
    await flushUi();

    emitMockEvent('session:status', {
      session_id: 'session-1',
      status: 'Connected',
      message: null,
    });
    emitMockEvent('terminal:data', {
      session_id: 'session-1',
      data: 'ready',
    });
    emitMockEvent('monitor:snapshot', makeSnapshot({ session_id: 'session-1', cpu_usage: 42.0 }));

    await flushUi();

    expect(invoke).toHaveBeenCalledWith('list_hosts');
    expect(invoke).toHaveBeenCalledWith('open_session', { hostId: 'host-1' });
    expect(invoke).toHaveBeenCalledWith('resize_terminal', {
      sessionId: 'session-1',
      cols: 120,
      rows: 32,
    });
    expect(wrapper.text()).toContain('已连接');
  });
});
