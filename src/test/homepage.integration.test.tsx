import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { invoke } from '@tauri-apps/api/core';
import { emitMockEvent, listen, resetMockEvents } from '@tauri-apps/api/event';
import HomePage from '@/pages/HomePage';
import { useHostStore } from '@/stores/host';
import { useLayoutStore } from '@/stores/layout';
import { useMonitorStore } from '@/stores/monitor';
import { useSessionStore } from '@/stores/session';
import { useSftpStore } from '@/stores/sftp';
import { SessionStatus } from '@/types/session';
import { makeHost, makeSession, makeSnapshot, makeTaskInfo } from './fixtures';

vi.mock('@/components/terminal/XtermView', () => ({ default: () => <div data-testid="xterm" /> }));
const mockInvoke = vi.mocked(invoke);
const mockListen = vi.mocked(listen);

/** 重置首页会访问的全局 store。 */
function resetStores() {
  useHostStore.setState(useHostStore.getInitialState(), true);
  useLayoutStore.setState(useLayoutStore.getInitialState(), true);
  useMonitorStore.setState(useMonitorStore.getInitialState(), true);
  useSessionStore.setState(useSessionStore.getInitialState(), true);
  useSftpStore.setState(useSftpStore.getInitialState(), true);
}

describe('HomePage integration', () => {
  beforeEach(() => {
    resetStores();
    resetMockEvents();
    mockInvoke.mockReset();
    mockInvoke.mockImplementation(async (command) => {
      if (command === 'list_hosts') return [makeHost()];
      if (command === 'open_session') return makeSession();
      if (command === 'start_monitoring') return makeTaskInfo();
      if (command === 'sftp_list_dir') return [];
      return undefined;
    });
  });

  it('加载主机并从首页打开真实会话', async () => {
    const user = userEvent.setup();
    render(<HomePage />);
    await user.click(await screen.findByText('root@10.0.0.8:22'));
    expect(mockInvoke).toHaveBeenCalledWith('open_session', { hostId: 'host-1' });
    expect(await screen.findByTestId('xterm')).toBeInTheDocument();
  });

  it('会话与监控事件更新标签和服务器状态', async () => {
    const user = userEvent.setup();
    render(<HomePage />);
    await user.click(await screen.findByText('root@10.0.0.8:22'));
    await act(async () => {
      emitMockEvent('session:status', { sessionId: 'session-1', status: SessionStatus.Connected, message: null });
      emitMockEvent('monitor:snapshot', makeSnapshot());
    });
    expect(screen.getByText('已连接')).toBeInTheDocument();
    expect(screen.getByText('21.5%')).toBeInTheDocument();
  });

  it('全局事件只由所属前端 module 监听一次', async () => {
    render(<HomePage />);
    await waitFor(() => expect(mockInvoke).toHaveBeenCalledWith('list_hosts'));
    const eventNames = mockListen.mock.calls.map(([eventName]) => eventName);

    expect(eventNames.filter((name) => name === 'session:status')).toHaveLength(1);
    expect(eventNames.filter((name) => name === 'monitor:snapshot')).toHaveLength(1);
    expect(eventNames.filter((name) => name === 'terminal:data')).toHaveLength(0);
  });

  it('拖动侧栏时更新并限制宽度', async () => {
    const { container } = render(<HomePage />);
    await waitFor(() => expect(mockInvoke).toHaveBeenCalledWith('list_hosts'));
    const resizer = container.querySelector('.sidebar-resizer')!;
    const start = new Event('pointerdown', { bubbles: true });
    Object.defineProperty(start, 'clientX', { value: 400 });
    fireEvent(resizer, start);
    const move = new Event('pointermove');
    Object.defineProperty(move, 'clientX', { value: 1 });
    fireEvent(window, move);
    expect(useLayoutStore.getState().sidebarWidth).toBe(220);
  });
});
