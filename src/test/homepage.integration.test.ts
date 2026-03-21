import { beforeEach, describe, expect, it, vi } from 'vitest';
import { flushPromises, mount } from '@vue/test-utils';
import { createPinia } from 'pinia';
import { nextTick } from 'vue';
import HomePage from '@/pages/HomePage.vue';
import { invoke } from '@tauri-apps/api/core';
import { emitMockEvent, resetMockEvents } from '@tauri-apps/api/event';
import { makeHost, makeSession, makeSnapshot, makeTaskInfo } from './fixtures';
import { SessionStatus } from '@/types/session';
import {
  DEFAULT_SIDEBAR_WIDTH,
  MAX_SIDEBAR_WIDTH,
  MIN_SIDEBAR_WIDTH,
} from '@/stores/layout';

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

function setViewportWidth(width: number) {
  Object.defineProperty(window, 'innerWidth', {
    configurable: true,
    writable: true,
    value: width,
  });
}

describe('HomePage integration', () => {
  beforeEach(() => {
    vi.mocked(invoke).mockReset();
    resetMockEvents();
    vi.stubGlobal('ResizeObserver', ResizeObserverMock);
    setViewportWidth(1280);
  });

  it('renders the sidebar with the default width', async () => {
    vi.mocked(invoke).mockResolvedValueOnce([]);

    const wrapper = mount(HomePage, {
      global: {
        plugins: [createPinia()],
        stubs: {
          Teleport: true,
        },
      },
    });

    await flushUi();

    const sidebar = wrapper.get('.sidebar').element as HTMLElement;
    expect(sidebar.style.width).toBe(`${DEFAULT_SIDEBAR_WIDTH}px`);
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

  it('leaves connecting state after a timeout status event', async () => {
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
    await wrapper.get('.host-card').trigger('click');
    await flushUi();

    expect(wrapper.text()).toContain('连接中');

    vi.mocked(invoke).mockResolvedValueOnce(undefined); // sync_session_status
    emitMockEvent('session:status', {
      session_id: 'session-1',
      status: SessionStatus.Timeout,
      message: 'Connection timeout after 10s',
    });

    await flushUi();

    expect(wrapper.text()).not.toContain('连接中');
    expect(wrapper.text()).toContain('离线');
    expect(invoke).toHaveBeenCalledWith('sync_session_status', {
      sessionId: 'session-1',
      status: SessionStatus.Timeout,
    });
  });

  it('updates the sidebar width when dragging the resize handle', async () => {
    vi.mocked(invoke).mockResolvedValueOnce([]);

    const wrapper = mount(HomePage, {
      attachTo: document.body,
      global: {
        plugins: [createPinia()],
        stubs: {
          Teleport: true,
        },
      },
    });

    await flushUi();

    await wrapper.get('.sidebar-resizer').trigger('pointerdown', { clientX: DEFAULT_SIDEBAR_WIDTH });
    window.dispatchEvent(new MouseEvent('pointermove', { clientX: 420 }));
    window.dispatchEvent(new MouseEvent('pointerup'));
    await flushUi();

    const sidebar = wrapper.get('.sidebar').element as HTMLElement;
    expect(sidebar.style.width).toBe('420px');
  });

  it('clamps the sidebar width to min and max boundaries during drag', async () => {
    vi.mocked(invoke).mockResolvedValueOnce([]);

    const wrapper = mount(HomePage, {
      attachTo: document.body,
      global: {
        plugins: [createPinia()],
        stubs: {
          Teleport: true,
        },
      },
    });

    await flushUi();

    await wrapper.get('.sidebar-resizer').trigger('pointerdown', { clientX: DEFAULT_SIDEBAR_WIDTH });
    window.dispatchEvent(new MouseEvent('pointermove', { clientX: MIN_SIDEBAR_WIDTH - 50 }));
    await flushUi();

    let sidebar = wrapper.get('.sidebar').element as HTMLElement;
    expect(sidebar.style.width).toBe(`${MIN_SIDEBAR_WIDTH}px`);

    window.dispatchEvent(new MouseEvent('pointermove', { clientX: MAX_SIDEBAR_WIDTH + 120 }));
    window.dispatchEvent(new MouseEvent('pointerup'));
    await flushUi();

    sidebar = wrapper.get('.sidebar').element as HTMLElement;
    expect(sidebar.style.width).toBe(`${MAX_SIDEBAR_WIDTH}px`);
  });
});
