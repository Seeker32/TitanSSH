import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { invoke } from '@tauri-apps/api/core';
import { emitMockEvent, listen, resetMockEvents } from '@tauri-apps/api/event';
import HomePage from '@/pages/HomePage';
import { useHostStore } from '@/stores/host';
import { useLayoutStore } from '@/stores/layout';
import { useLocaleStore } from '@/stores/locale';
import { useMonitorStore } from '@/stores/monitor';
import { useSessionStore } from '@/stores/session';
import { useSftpStore } from '@/stores/sftp';
import { useLogLevelStore } from '@/stores/log-level';
import { useTerminalThemeStore } from '@/stores/terminal-theme';
import { ConnectionPhase, SessionStatus } from '@/types/session';
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
  useLogLevelStore.setState(useLogLevelStore.getInitialState(), true);
  useTerminalThemeStore.setState(useTerminalThemeStore.getInitialState(), true);
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
      emitMockEvent('session:status', { sessionId: 'session-1', status: SessionStatus.Connected, error: null });
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
    expect(screen.getByRole('img', { name: '最近一分钟网卡速率趋势' })).toBeInTheDocument();
    expect(screen.getByLabelText('网卡接口')).toHaveValue('eth0');
    await user.selectOptions(screen.getByLabelText('网卡接口'), 'eth1');
    expect(screen.getByText('2.0 KB/s')).toBeInTheDocument();
    await act(async () => emitMockEvent('monitor:snapshot', makeSnapshot({ timestamp: 1_710_000_121_000, network: {
      available: true,
      interfaces: [
        { name: 'eth0', receiveBytesPerSecond: 1024, transmitBytesPerSecond: 512 },
        { name: 'eth1', receiveBytesPerSecond: 3072, transmitBytesPerSecond: 1536 },
      ],
    } })));
    expect(useMonitorStore.getState().networkTrends.get('session-1')).toEqual([
      { timestamp: 1_710_000_121_000, receiveBytesPerSecond: 3072, transmitBytesPerSecond: 1536 },
    ]);
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

  it('终端标签独立呈现连接阶段，后台会话错误不覆盖当前标签', async () => {
    const user = userEvent.setup();
    render(<HomePage />);
    await user.dblClick(await screen.findByTestId('host-card-host-1'));
    const overlay = screen.getByRole('status');
    expect(overlay).toHaveTextContent('正在连接 root@10.0.0.8');

    await act(async () => {
      emitMockEvent('session:progress', { sessionId: 'session-1', phase: ConnectionPhase.RequestingPty, timestamp: Date.now() });
    });
    expect(screen.getByRole('status')).toHaveTextContent('正在请求终端 PTY...');

    act(() => useSessionStore.setState({ sessions: new Map([
      ...useSessionStore.getState().sessions,
      ['session-2', makeSession({ sessionId: 'session-2' })],
    ]) }));
    await act(async () => {
      emitMockEvent('session:status', { sessionId: 'session-2', status: SessionStatus.AuthFailed, error: null });
    });

    // 当前标签仍呈现自己的连接阶段，后台标签仅更新状态点
    expect(screen.getByRole('status')).toHaveTextContent('正在请求终端 PTY...');
    const tabs = screen.getAllByRole('tab');
    expect(tabs[1].querySelector('.dot-error')).not.toBeNull();

    await act(async () => {
      emitMockEvent('session:status', { sessionId: 'session-1', status: SessionStatus.Connected, error: null });
    });
    expect(screen.queryByRole('status')).toBeNull();
  });

  it('连接失败在所属标签内显示错误，关闭标签调用后端 teardown', async () => {
    const user = userEvent.setup();
    render(<HomePage />);
    await user.dblClick(await screen.findByTestId('host-card-host-1'));
    await act(async () => {
      emitMockEvent('session:status', {
        sessionId: 'session-1', status: SessionStatus.Error,
        error: { code: 'SshConnectionError', detail: 'connection refused' },
      });
    });
    expect(screen.getByRole('alert')).toHaveTextContent('SSH 连接失败');
    expect(screen.getByRole('alert')).toHaveTextContent('connection refused');
    mockInvoke.mockClear();
    await user.click(screen.getByRole('button', { name: '关闭标签' }));
    await waitFor(() => expect(mockInvoke).toHaveBeenCalledWith('close_session', { sessionId: 'session-1' }));
    expect(screen.queryByRole('alert')).toBeNull();
  });

  it('英文环境下连接阶段与失败操作使用英文文案', async () => {
    const user = userEvent.setup();
    act(() => useLocaleStore.setState({ locale: 'en-US' }));
    render(<HomePage />);
    await user.dblClick(await screen.findByTestId('host-card-host-1'));
    expect(screen.getByRole('status')).toHaveTextContent('Connecting to root@10.0.0.8');
    await act(async () => {
      emitMockEvent('session:progress', { sessionId: 'session-1', phase: ConnectionPhase.ConnectingTcp, timestamp: Date.now() });
    });
    expect(screen.getByRole('status')).toHaveTextContent('Establishing TCP connection...');
    await act(async () => {
      emitMockEvent('session:status', { sessionId: 'session-1', status: SessionStatus.Timeout, error: null });
    });
    expect(screen.getByRole('alert')).toHaveTextContent('Connection timed out');
    expect(screen.getByRole('button', { name: 'Close Tab' })).toBeInTheDocument();
    act(() => useLocaleStore.setState({ locale: 'zh-CN' }));
  });

  it('首次主机身份确认：内联卡片、接受后续连、拒绝后关闭会话', async () => {
    const user = userEvent.setup();
    render(<HomePage />);
    await user.dblClick(await screen.findByTestId('host-card-host-1'));

    // 验证阶段进度与确认卡依次呈现；确认前终端不可交互
    await act(async () => {
      emitMockEvent('session:progress', { sessionId: 'session-1', phase: ConnectionPhase.VerifyingHostKey, timestamp: Date.now() });
      emitMockEvent('host-identity:challenge', {
        challengeId: 'challenge-1', sessionId: 'session-1', host: '10.0.0.8', port: 22,
        keyAlgorithm: 'ssh-ed25519', fingerprint: 'SHA256:ungWv48Bz+pBQUDeXa4iI7ADYaOWF3qctBD', timestamp: 1_710_000_000_000,
      });
    });
    const card = screen.getByTestId('host-identity-card');
    expect(card).toHaveTextContent('10.0.0.8:22');
    expect(card).toHaveTextContent('ssh-ed25519');
    expect(card).toHaveTextContent('SHA256:ungWv48Bz+pBQUDeXa4iI7ADYaOWF3qctBD');
    expect(screen.queryByRole('status')).toBeNull();

    mockInvoke.mockClear();
    mockInvoke.mockResolvedValue(undefined);
    await user.click(screen.getByRole('button', { name: '仅本次接受' }));
    await waitFor(() => expect(mockInvoke).toHaveBeenCalledWith('accept_host_identity', { challengeId: 'challenge-1' }));
    expect(screen.queryByTestId('host-identity-card')).toBeNull();

    // 拒绝路径：新 challenge 拒绝后关闭整个会话
    await act(async () => {
      emitMockEvent('host-identity:challenge', {
        challengeId: 'challenge-2', sessionId: 'session-1', host: '10.0.0.8', port: 22,
        keyAlgorithm: 'ssh-ed25519', fingerprint: 'SHA256:another', timestamp: 1_710_000_001_000,
      });
    });
    mockInvoke.mockClear();
    // 后端拒绝时已 teardown，close_session 可能报错（会话已移除）
    mockInvoke.mockImplementation(async (command) => {
      if (command === 'reject_host_identity') return undefined;
      throw new Error('SessionNotFound');
    });
    await user.click(screen.getByRole('button', { name: '拒绝' }));
    await waitFor(() => expect(mockInvoke).toHaveBeenCalledWith('reject_host_identity', { challengeId: 'challenge-2' }));
    expect(mockInvoke).toHaveBeenCalledWith('close_session', { sessionId: 'session-1' });
    await waitFor(() => expect(useSessionStore.getState().sessions.has('session-1')).toBe(false));
    expect(screen.queryByTestId('host-identity-card')).toBeNull();
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

  it('设置对话框通过左侧导航切换内容，并保存日志等级', async () => {
    const user = userEvent.setup();
    render(<HomePage />);
    await user.click(await screen.findByRole('button', { name: '设置' }));
    const dialog = screen.getByRole('dialog', { name: '设置' });
    expect(within(dialog).getByTestId('settings-section-general')).toBeInTheDocument();
    await user.click(within(dialog).getByTestId('settings-section-terminal'));
    expect(within(dialog).getAllByRole('button', { name: /SSH 终端主题/ })).toHaveLength(6);
    const applicationTheme = document.documentElement.dataset.theme;
    await user.click(within(dialog).getByRole('button', { name: /Dracula/ }));
    expect(useTerminalThemeStore.getState().terminalTheme).toBe('dracula');
    expect(localStorage.getItem('terminal-theme')).toBe('dracula');
    expect(document.documentElement.dataset.theme).toBe(applicationTheme);
    await user.click(within(dialog).getByTestId('settings-section-logging'));
    await user.selectOptions(within(dialog).getByLabelText('日志等级'), 'debug');
    await waitFor(() => expect(mockInvoke).toHaveBeenCalledWith('set_log_level', { level: 'debug' }));
    expect(useLogLevelStore.getState().logLevel).toBe('debug');
    expect(localStorage.getItem('log-level')).toBe('debug');
  });

  it('侧栏拖拽区同时承载尺寸调整光标与拖动事件', async () => {
    const { container } = render(<HomePage />);
    await waitFor(() => expect(mockInvoke).toHaveBeenCalledWith('list_hosts'));
    const resizer = screen.getByTestId('sidebar-resizer');
    expect(resizer).toHaveClass('sidebar-resizer');
    expect(resizer).toHaveAttribute('aria-orientation', 'vertical');
    const sidebar = container.querySelector('.sidebar')!;
    const start = new Event('pointerdown', { bubbles: true });
    Object.defineProperty(start, 'clientX', { value: 300 });
    fireEvent(resizer, start);
    const move = new Event('pointermove');
    Object.defineProperty(move, 'clientX', { value: 1 });
    fireEvent(window, move);
    expect(useLayoutStore.getState().sidebarWidth).toBe(220);
  });

  it('侧栏拖动阻止文本选中：阻止默认行为并仅拖动期间加禁选类', async () => {
    render(<HomePage />);
    await waitFor(() => expect(mockInvoke).toHaveBeenCalledWith('list_hosts'));
    const resizer = screen.getByTestId('sidebar-resizer');
    const start = new Event('pointerdown', { bubbles: true, cancelable: true });
    const preventDefault = vi.spyOn(start, 'preventDefault');
    Object.defineProperty(start, 'clientX', { value: 300 });
    fireEvent(resizer, start);
    expect(preventDefault).toHaveBeenCalled();
    expect(document.body.classList.contains('sidebar-resizing')).toBe(true);
    fireEvent(window, new Event('pointerup'));
    expect(document.body.classList.contains('sidebar-resizing')).toBe(false);
  });
});
