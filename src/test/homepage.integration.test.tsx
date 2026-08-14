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
    // 等待确认期间 Session 保持 Connecting，并在确认卡内展示主机身份验证阶段
    expect(card).toHaveTextContent('正在验证主机身份...');
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
    mockInvoke.mockResolvedValue(undefined);
    await user.click(screen.getByRole('button', { name: '拒绝' }));
    await waitFor(() => expect(mockInvoke).toHaveBeenCalledWith('reject_host_identity', { challengeId: 'challenge-2' }));
    // 后端在拒绝命令内 teardown，前端不得重复 close_session
    expect(mockInvoke).not.toHaveBeenCalledWith('close_session', { sessionId: 'session-1' });
    await waitFor(() => expect(useSessionStore.getState().sessions.has('session-1')).toBe(false));
    expect(screen.queryByTestId('host-identity-card')).toBeNull();
  });

  it('接受并保存：调用后端命令；保存失败保持确认卡并展示结构化错误，可改选仅本次接受', async () => {
    const user = userEvent.setup();
    render(<HomePage />);
    await user.dblClick(await screen.findByTestId('host-card-host-1'));
    await act(async () => {
      emitMockEvent('host-identity:challenge', {
        challengeId: 'challenge-save', sessionId: 'session-1', host: '10.0.0.8', port: 22,
        keyAlgorithm: 'ssh-ed25519', fingerprint: 'SHA256:ungWv48Bz+pBQUDeXa4iI7ADYaOWF3qctBD', timestamp: 1_710_000_000_000,
      });
    });

    // 保存失败：结构化错误显示在所属标签的确认卡内，challenge 保持未决
    mockInvoke.mockRejectedValueOnce({ code: 'HostKeySaveFailed', detail: 'write denied' });
    await user.click(screen.getByRole('button', { name: '接受并保存' }));
    await waitFor(() => expect(mockInvoke).toHaveBeenCalledWith('accept_and_save_host_identity', { challengeId: 'challenge-save' }));
    const card = screen.getByTestId('host-identity-card');
    expect(within(card).getByTestId('host-identity-save-error')).toHaveTextContent('主机信任保存失败: write denied');
    expect(within(card).getByRole('button', { name: '接受并保存' })).toBeInTheDocument();
    // 失败绝不自动降级为临时信任
    expect(mockInvoke).not.toHaveBeenCalledWith('accept_host_identity', expect.anything());

    // 改选仅本次接受：确认卡与错误一并清除
    mockInvoke.mockResolvedValue(undefined);
    await user.click(screen.getByRole('button', { name: '仅本次接受' }));
    await waitFor(() => expect(mockInvoke).toHaveBeenCalledWith('accept_host_identity', { challengeId: 'challenge-save' }));
    expect(screen.queryByTestId('host-identity-card')).toBeNull();
    expect(useSessionStore.getState().hostKeySaveErrors.has('session-1')).toBe(false);
  });

  it('主机身份变更：内联卡展示新旧指纹，替换记录需二次确认，失败可改选仅本次接受', async () => {
    const user = userEvent.setup();
    render(<HomePage />);
    await user.dblClick(await screen.findByTestId('host-card-host-1'));
    await act(async () => {
      emitMockEvent('host-identity:challenge', {
        challengeId: 'challenge-changed', sessionId: 'session-1', host: '10.0.0.8', port: 22,
        kind: 'Changed',
        keyAlgorithm: 'ssh-rsa', fingerprint: 'SHA256:newfp',
        storedAlgorithm: 'ssh-ed25519', storedFingerprint: 'SHA256:oldfp',
        timestamp: 1_710_000_000_000,
      });
    });
    const card = screen.getByTestId('host-identity-card');
    expect(card).toHaveTextContent('主机身份已变更');
    expect(within(card).getByTestId('host-identity-stored')).toHaveTextContent('SHA256:oldfp');
    expect(within(card).getByTestId('host-identity-presented')).toHaveTextContent('SHA256:newfp');

    // 替换记录必须先经过第二次内联确认
    mockInvoke.mockRejectedValueOnce({ code: 'HostKeySaveFailed', detail: 'write denied' });
    await user.click(screen.getByTestId('host-identity-replace'));
    await user.click(screen.getByTestId('host-identity-replace-confirm-btn'));
    await waitFor(() => expect(mockInvoke).toHaveBeenCalledWith('accept_and_save_host_identity', { challengeId: 'challenge-changed' }));
    // 替换写入失败：确认卡保持未决并展示替换失败文案，不降级为临时信任
    expect(within(card).getByTestId('host-identity-save-error')).toHaveTextContent('write denied');
    expect(mockInvoke).not.toHaveBeenCalledWith('accept_host_identity', expect.anything());

    // 明确改选仅本次接受：正常解决并清除确认卡
    mockInvoke.mockResolvedValue(undefined);
    await user.click(screen.getByRole('button', { name: '仅本次接受' }));
    await waitFor(() => expect(mockInvoke).toHaveBeenCalledWith('accept_host_identity', { challengeId: 'challenge-changed' }));
    expect(screen.queryByTestId('host-identity-card')).toBeNull();
  });

  it('保存成功后新 Session 不再提示：连接进入 Connected 时清理确认卡投影', async () => {
    const user = userEvent.setup();
    render(<HomePage />);
    await user.dblClick(await screen.findByTestId('host-card-host-1'));
    await act(async () => {
      emitMockEvent('host-identity:challenge', {
        challengeId: 'challenge-save', sessionId: 'session-1', host: '10.0.0.8', port: 22,
        keyAlgorithm: 'ssh-ed25519', fingerprint: 'SHA256:ungWv48Bz+pBQUDeXa4iI7ADYaOWF3qctBD', timestamp: 1_710_000_000_000,
      });
    });
    mockInvoke.mockResolvedValue(undefined);
    await user.click(screen.getByRole('button', { name: '接受并保存' }));
    await waitFor(() => expect(mockInvoke).toHaveBeenCalledWith('accept_and_save_host_identity', { challengeId: 'challenge-save' }));
    expect(screen.queryByTestId('host-identity-card')).toBeNull();

    // 其他 Session 的同 key pending challenge 被后端保存自动放行：进入 Connected 后清理确认卡
    await act(async () => {
      emitMockEvent('host-identity:challenge', {
        challengeId: 'challenge-cross', sessionId: 'session-1', host: '10.0.0.8', port: 22,
        keyAlgorithm: 'ssh-ed25519', fingerprint: 'SHA256:ungWv48Bz+pBQUDeXa4iI7ADYaOWF3qctBD', timestamp: 1_710_000_001_000,
      });
    });
    expect(screen.getByTestId('host-identity-card')).toBeVisible();
    await act(async () => {
      emitMockEvent('session:status', { sessionId: 'session-1', status: SessionStatus.Connected, error: null });
    });
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

  it('保存主机失败时展示错误并保持编辑弹窗打开', async () => {
    const user = userEvent.setup();
    mockInvoke.mockImplementation(async (command) => {
      if (command === 'list_hosts') return [makeHost()];
      if (command === 'save_host') throw { code: 'SecureStoreError', detail: 'The name org.freedesktop.secrets was not provided by any .service files' };
      return undefined;
    });
    const { container } = render(<HomePage />);
    const emptyState = container.querySelector('.empty-state') as HTMLElement;
    await user.click(within(emptyState).getByRole('button', { name: '新建主机' }));
    await user.type(screen.getByPlaceholderText('生产服务器'), 'prod');
    await user.type(screen.getByPlaceholderText('192.168.1.12'), '10.0.0.8');
    await user.type(screen.getByPlaceholderText('root'), 'root');
    await user.type(screen.getByPlaceholderText('留空则保持原密码不变'), 'secret');
    await user.click(screen.getByRole('button', { name: '保存连接' }));

    expect(await screen.findByTestId('host-editor-save-error'))
      .toHaveTextContent('安全存储错误: The name org.freedesktop.secrets was not provided by any .service files');
    expect(screen.getByText('新建连接')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '保存连接' })).toBeInTheDocument();
  });

  it('保存主机成功后关闭编辑弹窗', async () => {
    const user = userEvent.setup();
    mockInvoke.mockImplementation(async (command) => {
      if (command === 'list_hosts') return [makeHost()];
      if (command === 'save_host') return [makeHost()];
      return undefined;
    });
    const { container } = render(<HomePage />);
    const emptyState = container.querySelector('.empty-state') as HTMLElement;
    await user.click(within(emptyState).getByRole('button', { name: '新建主机' }));
    await user.type(screen.getByPlaceholderText('生产服务器'), 'prod');
    await user.type(screen.getByPlaceholderText('192.168.1.12'), '10.0.0.8');
    await user.type(screen.getByPlaceholderText('root'), 'root');
    await user.type(screen.getByPlaceholderText('留空则保持原密码不变'), 'secret');
    await user.click(screen.getByRole('button', { name: '保存连接' }));

    await waitFor(() => expect(screen.queryByText('新建连接')).not.toBeInTheDocument());
  });

  it('语言选择器选项始终以语言自身名称展示', async () => {
    const user = userEvent.setup();
    useLocaleStore.getState().setLocale('en-US');
    render(<HomePage />);
    await user.click(await screen.findByRole('button', { name: 'Settings' }));
    await user.click(screen.getByRole('combobox'));
    // 语言名使用自身语言（endonym）：英语界面下中文选项仍显示“简体中文”
    expect(await screen.findByRole('option', { name: '简体中文' })).toBeInTheDocument();
    expect(screen.getByRole('option', { name: 'English' })).toBeInTheDocument();
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

  it('终端主题卡片从标题下方左侧开始排列，标题不占网格首格', async () => {
    const user = userEvent.setup();
    render(<HomePage />);
    await user.click(await screen.findByRole('button', { name: '设置' }));
    const dialog = screen.getByRole('dialog', { name: '设置' });
    await user.click(within(dialog).getByTestId('settings-section-terminal'));
    const grid = document.querySelector('.terminal-theme-options') as HTMLElement;
    expect(grid).not.toBeNull();
    // 标题若作为网格子元素会占据首格，把第一张卡片挤到右侧；网格子元素必须全部是主题卡片
    const children = [...grid.children];
    expect(children).toHaveLength(6);
    for (const child of children) {
      expect(child.tagName).toBe('BUTTON');
      expect(child).toHaveClass('terminal-theme-card');
    }
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
