import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
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

  it('加载主机并从侧栏双击打开真实会话', async () => {
    const user = userEvent.setup();
    render(<HomePage />);
    await user.dblClick(await screen.findByTestId('host-card-host-1'));
    expect(mockInvoke).toHaveBeenCalledWith('open_session', { hostId: 'host-1' });
    expect(await screen.findByTestId('xterm')).toBeInTheDocument();
  });

  it('会话与监控事件更新标签和服务器状态', async () => {
    const user = userEvent.setup();
    render(<HomePage />);
    await user.dblClick(await screen.findByTestId('host-card-host-1'));
    await act(async () => {
      emitMockEvent('session:status', { sessionId: 'session-1', status: SessionStatus.Connected, message: null });
      emitMockEvent('monitor:snapshot', makeSnapshot({ network: {
        available: true,
        interfaces: [
          { name: 'eth0', receiveBytesPerSecond: 1024, transmitBytesPerSecond: 512 },
          { name: 'eth1', receiveBytesPerSecond: 2048, transmitBytesPerSecond: 1024 },
        ],
      } }));
    });
    expect(screen.getByText('已连接')).toBeInTheDocument();
    expect(screen.getByText('21.5%')).toBeInTheDocument();
    expect(screen.getByText('1.0 KB/s')).toBeInTheDocument();
    expect(screen.getByLabelText('网卡接口')).toHaveValue('eth0');
    await user.selectOptions(screen.getByLabelText('网卡接口'), 'eth1');
    expect(screen.getByText('2.0 KB/s')).toBeInTheDocument();
  });

  it('切换活动会话后保留各自的网卡选择', async () => {
    const user = userEvent.setup();
    render(<HomePage />);
    act(() => useSessionStore.setState({ sessions: new Map([
      ['session-1', makeSession()],
      ['session-2', makeSession({ sessionId: 'session-2' })],
    ]), activeView: 'session-1' }));
    await act(async () => {
      emitMockEvent('monitor:snapshot', makeSnapshot({ network: {
        available: true,
        interfaces: [
          { name: 'eth0', receiveBytesPerSecond: 1024, transmitBytesPerSecond: 512 },
          { name: 'eth1', receiveBytesPerSecond: 2048, transmitBytesPerSecond: 1024 },
        ],
      } }));
      emitMockEvent('monitor:snapshot', makeSnapshot({ sessionId: 'session-2', network: {
        available: true,
        interfaces: [{ name: 'ens5', receiveBytesPerSecond: 4096, transmitBytesPerSecond: 2048 }],
      } }));
    });
    await user.selectOptions(screen.getByLabelText('网卡接口'), 'eth1');
    act(() => useSessionStore.getState().setActiveView('session-2'));
    expect(screen.getByLabelText('网卡接口')).toHaveValue('ens5');
    act(() => useSessionStore.getState().setActiveView('session-1'));
    expect(screen.getByLabelText('网卡接口')).toHaveValue('eth1');
  });

  it('无会话时主区显示空态页，新建按钮打开编辑器', async () => {
    const user = userEvent.setup();
    const { container } = render(<HomePage />);
    await waitFor(() => expect(mockInvoke).toHaveBeenCalledWith('list_hosts'));
    const emptyState = container.querySelector('.empty-state') as HTMLElement;
    expect(within(emptyState).getByText(/选择左侧主机/)).toBeInTheDocument();
    await user.click(within(emptyState).getByRole('button', { name: '新建主机' }));
    expect(screen.getByText('新建连接')).toBeInTheDocument();
  });

  it('全局事件只由所属前端 module 监听一次', async () => {
    render(<HomePage />);
    await waitFor(() => expect(mockInvoke).toHaveBeenCalledWith('list_hosts'));
    const eventNames = mockListen.mock.calls.map(([eventName]) => eventName);

    expect(eventNames.filter((name) => name === 'session:status')).toHaveLength(1);
    expect(eventNames.filter((name) => name === 'monitor:snapshot')).toHaveLength(1);
    expect(eventNames.filter((name) => name === 'terminal:data')).toHaveLength(0);
  });

  it('主题切换按钮使用 lucide 图标并可切换主题', async () => {
    const user = userEvent.setup();
    render(<HomePage />);
    await waitFor(() => expect(mockInvoke).toHaveBeenCalledWith('list_hosts'));
    const toggle = document.querySelector('[data-testid="theme-toggle"]')!;
    expect(toggle.querySelector('svg')).not.toBeNull();
    const before = document.documentElement.dataset.theme;
    await user.click(toggle);
    expect(document.documentElement.dataset.theme).not.toBe(before);
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
