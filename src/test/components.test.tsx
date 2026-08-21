import { open as openFileDialog } from '@tauri-apps/plugin-dialog';
import { act, fireEvent, render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import FileExplorer from '@/components/sftp/FileExplorer';
import RecentTransfers from '@/components/sftp/RecentTransfers';
import HostEditorDialog from '@/components/host/HostEditorDialog';
import HostListSidebar from '@/components/host/HostListSidebar';
import { groupHosts } from '@/stores/host';
import ServerStatusPanel from '@/components/status/ServerStatusPanel';
import SftpPanel from '@/components/sftp/SftpPanel';
import TerminalPane from '@/components/terminal/TerminalPane';
import TerminalTabs from '@/components/terminal/TerminalTabs';
import HostIdentityCard from '@/components/terminal/HostIdentityCard';
import TransferQueue from '@/components/sftp/TransferQueue';
import { AuthType } from '@/types/host';
import { ConnectionPhase, SessionStatus, type HostIdentityChallenge } from '@/types/session';
import { useLocaleStore } from '@/stores/locale';
import { makeHost, makeRemoteDir, makeRemoteEntry, makeSession, makeSnapshot, makeTransferTask } from './fixtures';

vi.mock('@/components/terminal/XtermView', () => ({
  default: ({ sessionId, active, interactive }: { sessionId: string; active: boolean; interactive?: boolean }) => (
    <div data-testid="xterm" data-session={sessionId} data-interactive={String(interactive ?? true)} hidden={!active} />
  ),
}));

describe('React components', () => {
  it('近期传输默认折叠，展开显示终态并可清空', async () => {
    const user = userEvent.setup();
    const onClear = vi.fn();
    render(<RecentTransfers tasks={[makeTransferTask({ status: 'Failed', error: { code: 'SftpTransferError', detail: 'closed' } })]} onClear={onClear} />);

    expect(screen.queryByText('syslog')).not.toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: /近期传输/ }));
    expect(screen.getByText('syslog')).toBeInTheDocument();
    expect(screen.getByText('失败')).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: /清空近期记录/ }));
    expect(onClear).toHaveBeenCalledOnce();
  });

  it('侧栏主机卡片渲染服务器图标，单击选中、双击连接', async () => {
    const user = userEvent.setup();
    const handlers = { onSearchChange: vi.fn(), onSelect: vi.fn(), onOpen: vi.fn(), onCreate: vi.fn() };
    render(<HostListSidebar hosts={[makeHost()]} searchQuery="" selectedHostId={null} collapsedGroups={[]}
      onToggleGroup={vi.fn()} onRenameGroup={vi.fn()} onDeleteGroup={vi.fn()} onEditHost={vi.fn()} onDeleteHost={vi.fn()} {...handlers} />);
    const card = screen.getByText('prod').closest('[role="button"]')!;
    expect(card.querySelector('svg')).not.toBeNull();
    await user.click(screen.getByText('prod'));
    expect(handlers.onSelect).toHaveBeenCalledWith('host-1');
    expect(handlers.onOpen).not.toHaveBeenCalled();
    await user.dblClick(screen.getByText('prod'));
    expect(handlers.onOpen).toHaveBeenCalledWith('host-1');
    expect(screen.getByText('root@10.0.0.8:22')).toBeInTheDocument();
  });

  it('侧栏搜索可过滤并按结果展示空态', async () => {
    const user = userEvent.setup();
    const handlers = { onSearchChange: vi.fn(), onSelect: vi.fn(), onOpen: vi.fn(), onCreate: vi.fn() };
    const { rerender } = render(<HostListSidebar hosts={[makeHost()]} searchQuery="" selectedHostId={null} collapsedGroups={[]}
      onToggleGroup={vi.fn()} onRenameGroup={vi.fn()} onDeleteGroup={vi.fn()} onEditHost={vi.fn()} onDeleteHost={vi.fn()} {...handlers} />);
    await user.type(screen.getByPlaceholderText('搜索主机…'), 'prod');
    expect(handlers.onSearchChange).toHaveBeenCalledWith('p');
    rerender(<HostListSidebar hosts={[]} searchQuery="prod" selectedHostId={null} collapsedGroups={[]}
      onToggleGroup={vi.fn()} onRenameGroup={vi.fn()} onDeleteGroup={vi.fn()} onEditHost={vi.fn()} onDeleteHost={vi.fn()} {...handlers} />);
    expect(screen.getByText('未找到匹配的主机')).toBeInTheDocument();
  });

  it('无主机时显示新建引导并可打开编辑器', async () => {
    const user = userEvent.setup();
    const onCreate = vi.fn();
    render(<HostListSidebar hosts={[]} searchQuery="" selectedHostId={null}
      onSearchChange={vi.fn()} onSelect={vi.fn()} onOpen={vi.fn()} onCreate={onCreate} />);
    expect(screen.getByText(/暂无主机/)).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: '新建第一个主机' }));
    expect(onCreate).toHaveBeenCalledOnce();
  });

  it('侧栏按分组渲染且未分组排最后，分组头折叠展开', async () => {
    const user = userEvent.setup();
    const onToggleGroup = vi.fn();
    const hosts = [makeHost({ group: '' }), makeHost({ id: 'h2', name: 'staging', group: 'prod-env' })];
    const { container, rerender } = render(<HostListSidebar hosts={hosts} searchQuery="" selectedHostId={null}
      collapsedGroups={[]} onToggleGroup={onToggleGroup} onRenameGroup={vi.fn()} onDeleteGroup={vi.fn()} onEditHost={vi.fn()} onDeleteHost={vi.fn()}
      onSearchChange={vi.fn()} onSelect={vi.fn()} onOpen={vi.fn()} onCreate={vi.fn()} />);
    const headers = container.querySelectorAll('.host-group-header');
    expect(headers).toHaveLength(2);
    expect(headers[0].textContent).toContain('prod-env');
    expect(headers[1].textContent).toContain('未分组');
    await user.click(screen.getByTestId('group-header-prod-env'));
    expect(onToggleGroup).toHaveBeenCalledWith('prod-env');
    rerender(<HostListSidebar hosts={hosts} searchQuery="" selectedHostId={null} collapsedGroups={['prod-env']}
      onToggleGroup={onToggleGroup} onRenameGroup={vi.fn()} onDeleteGroup={vi.fn()} onEditHost={vi.fn()} onDeleteHost={vi.fn()} onSearchChange={vi.fn()} onSelect={vi.fn()} onOpen={vi.fn()} onCreate={vi.fn()} />);
    expect(screen.queryByText('staging')).not.toBeInTheDocument();
    expect(screen.getByText('prod')).toBeInTheDocument();
  });

  it('搜索时以平铺列表展示，不显示分组头', () => {
    render(<HostListSidebar hosts={[makeHost()]} searchQuery="prod" selectedHostId={null} collapsedGroups={[]}
      onToggleGroup={vi.fn()} onRenameGroup={vi.fn()} onDeleteGroup={vi.fn()} onEditHost={vi.fn()} onDeleteHost={vi.fn()} onSearchChange={vi.fn()} onSelect={vi.fn()} onOpen={vi.fn()} onCreate={vi.fn()} />);
    expect(screen.queryByText('未分组')).not.toBeInTheDocument();
    expect(screen.getByText('prod')).toBeInTheDocument();
  });

  it('分组头 hover 显示重命名/删除，未分组无操作，重命名行内编辑提交', async () => {
    const user = userEvent.setup();
    const onRenameGroup = vi.fn();
    const onDeleteGroup = vi.fn();
    const hosts = [makeHost({ group: '' }), makeHost({ id: 'h2', name: 'staging', group: 'prod-env' })];
    render(<HostListSidebar hosts={hosts} searchQuery="" selectedHostId={null} collapsedGroups={[]}
      onToggleGroup={vi.fn()} onEditHost={vi.fn()} onDeleteHost={vi.fn()} onRenameGroup={onRenameGroup} onDeleteGroup={onDeleteGroup}
      onSearchChange={vi.fn()} onSelect={vi.fn()} onOpen={vi.fn()} onCreate={vi.fn()} />);
    const group = screen.getByTestId('group-header-prod-env');
    await user.hover(group);
    await user.click(screen.getByTestId('group-rename-btn'));
    const input = screen.getByDisplayValue('prod-env');
    await user.clear(input);
    await user.type(input, 'prod-eu');
    await user.keyboard('{Enter}');
    expect(onRenameGroup).toHaveBeenCalledWith('prod-env', 'prod-eu');
    await user.hover(group);
    await user.click(screen.getByTestId('group-delete-btn'));
    expect(onDeleteGroup).toHaveBeenCalledWith('prod-env');
    const ungrouped = screen.getByTestId('group-header-ungrouped');
    await user.hover(ungrouped);
    expect(within(ungrouped).queryByTestId('group-delete-btn')).not.toBeInTheDocument();
    expect(within(ungrouped).queryByTestId('group-rename-btn')).not.toBeInTheDocument();
  });

  it('主机卡片 hover 提供编辑与删除操作', async () => {
    const user = userEvent.setup();
    const onEdit = vi.fn();
    const onDelete = vi.fn();
    render(<HostListSidebar hosts={[makeHost()]} searchQuery="" selectedHostId={null} collapsedGroups={[]}
      onToggleGroup={vi.fn()} onRenameGroup={vi.fn()} onDeleteGroup={vi.fn()} onEditHost={onEdit} onDeleteHost={onDelete}
      onSearchChange={vi.fn()} onSelect={vi.fn()} onOpen={vi.fn()} onCreate={vi.fn()} />);
    const card = screen.getByTestId('host-card-host-1');
    await user.hover(card);
    await user.click(screen.getByTestId('host-edit-btn'));
    expect(onEdit).toHaveBeenCalledWith('host-1');
    await user.click(screen.getByTestId('host-delete-btn'));
    expect(onDelete).toHaveBeenCalledWith('host-1');
  });

  it('分组函数：空分组不产生分组头', () => {
    expect(groupHosts([])).toEqual([]);
    expect(groupHosts([makeHost({ group: '' })])).toEqual([{ name: '', hosts: [makeHost({ group: '' })] }]);
  });

  it('终端标签仅渲染会话，激活切换并关闭', async () => {
    const user = userEvent.setup();
    const activate = vi.fn();
    const close = vi.fn();
    render(<TerminalTabs sessions={[makeSession({ status: SessionStatus.Connected })]} activeView="session-1" onActivate={activate} onClose={close} />);
    expect(screen.getAllByRole('tab')).toHaveLength(1);
    expect(screen.queryByText('首页')).not.toBeInTheDocument();
    expect(document.querySelector('.dot-connected')).toBeInTheDocument();
    await user.click(screen.getByText('root@10.0.0.8'));
    await user.click(screen.getByLabelText('关闭 root@10.0.0.8'));
    expect(activate).toHaveBeenCalledWith('session-1');
    expect(close).toHaveBeenCalledWith('session-1');
  });

  it('终端面板保留每个会话实例，仅展示当前视图', () => {
    render(<TerminalPane sessions={[makeSession(), makeSession({ sessionId: 'session-2' })]} activeView="session-2"
      connections={new Map()} challenges={new Map()} onInput={vi.fn()} onResize={vi.fn()} onCreateHost={vi.fn()} onCloseTab={vi.fn()} onSaveIdentity={vi.fn()} saveErrors={new Map()} onAcceptIdentity={vi.fn()} onRejectIdentity={vi.fn()} />);
    const terminals = screen.getAllByTestId('xterm');
    expect(terminals).toHaveLength(2);
    expect(terminals[0]).not.toBeVisible();
    expect(terminals[1]).toBeVisible();
  });

  it('无会话时终端面板显示空态页并可新建主机', () => {
    const onCreateHost = vi.fn();
    render(<TerminalPane sessions={[]} activeView={null} connections={new Map()} challenges={new Map()} onInput={vi.fn()} onResize={vi.fn()} onCreateHost={onCreateHost} onCloseTab={vi.fn()} onSaveIdentity={vi.fn()} saveErrors={new Map()} onAcceptIdentity={vi.fn()} onRejectIdentity={vi.fn()} />);
    expect(screen.getByText(/选择左侧主机/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: '新建主机' }));
    expect(onCreateHost).toHaveBeenCalledOnce();
  });

  it('连接中的会话在终端区域显示加载动画与当前阶段', () => {
    render(<TerminalPane sessions={[makeSession()]} activeView="session-1"
      connections={new Map([['session-1', { phase: ConnectionPhase.SshHandshake, error: null }]])}
      challenges={new Map()} onInput={vi.fn()} onResize={vi.fn()} onCreateHost={vi.fn()} onCloseTab={vi.fn()} onSaveIdentity={vi.fn()} saveErrors={new Map()} onAcceptIdentity={vi.fn()} onRejectIdentity={vi.fn()} />);
    const overlay = screen.getByRole('status');
    expect(overlay).toBeVisible();
    expect(overlay).toHaveTextContent('正在进行 SSH 握手...');
    expect(overlay.querySelector('.spinner')).not.toBeNull();
    // 连接未完成时不提供任何操作按钮，也不接收输入
    expect(screen.queryByRole('button')).toBeNull();
    expect(screen.getByTestId('xterm')).not.toHaveAttribute('data-interactive', 'true');
  });

  it('连接失败的会话显示结构化错误且仅提供关闭标签操作', async () => {
    const user = userEvent.setup();
    const onCloseTab = vi.fn();
    render(<TerminalPane sessions={[makeSession({ status: SessionStatus.Error })]} activeView="session-1"
      connections={new Map([['session-1', { phase: null, error: { code: 'SshConnectionError', detail: 'connection refused' } }]])}
      challenges={new Map()} onInput={vi.fn()} onResize={vi.fn()} onCreateHost={vi.fn()} onCloseTab={onCloseTab}
      onSaveIdentity={vi.fn()} saveErrors={new Map()} />);
    const alert = screen.getByRole('alert');
    expect(alert).toHaveTextContent('SSH 连接失败: connection refused');
    await user.click(screen.getByRole('button', { name: '关闭标签' }));
    expect(onCloseTab).toHaveBeenCalledWith('session-1');
  });

  it('两个会话各自呈现连接阶段且互不覆盖，非当前标签不抢占焦点', () => {
    render(<TerminalPane
      sessions={[
        makeSession({ sessionId: 'session-1' }),
        makeSession({ sessionId: 'session-2', status: SessionStatus.AuthFailed }),
      ]}
      activeView="session-1"
      connections={new Map([
        ['session-1', { phase: ConnectionPhase.ConnectingTcp, error: null }],
        ['session-2', { phase: null, error: null }],
      ])}
      challenges={new Map()} onInput={vi.fn()} onResize={vi.fn()} onCreateHost={vi.fn()} onCloseTab={vi.fn()} onSaveIdentity={vi.fn()} saveErrors={new Map()} onAcceptIdentity={vi.fn()} onRejectIdentity={vi.fn()} />);
    const overlays = screen.getAllByRole('status').length + screen.getAllByRole('alert', { hidden: true }).length;
    expect(overlays).toBe(2);
    expect(screen.getByRole('status')).toHaveTextContent('正在建立 TCP 连接...');
    expect(screen.getByRole('alert', { hidden: true })).not.toBeVisible();
    expect(screen.getByRole('status')).toBeVisible();
  });

  it('连接中切换语言后覆盖层文案即时更新', () => {
    const { rerender } = render(<TerminalPane sessions={[makeSession()]} activeView="session-1"
      connections={new Map([['session-1', { phase: ConnectionPhase.SshHandshake, error: null }]])}
      challenges={new Map()} onInput={vi.fn()} onResize={vi.fn()} onCreateHost={vi.fn()} onCloseTab={vi.fn()} onSaveIdentity={vi.fn()} saveErrors={new Map()} onAcceptIdentity={vi.fn()} onRejectIdentity={vi.fn()} />);
    expect(screen.getByRole('status')).toHaveTextContent('正在进行 SSH 握手...');
    act(() => useLocaleStore.setState({ locale: 'en-US' }));
    rerender(<TerminalPane sessions={[makeSession()]} activeView="session-1"
      connections={new Map([['session-1', { phase: ConnectionPhase.SshHandshake, error: null }]])}
      challenges={new Map()} onInput={vi.fn()} onResize={vi.fn()} onCreateHost={vi.fn()} onCloseTab={vi.fn()} onSaveIdentity={vi.fn()} saveErrors={new Map()} onAcceptIdentity={vi.fn()} onRejectIdentity={vi.fn()} />);
    expect(screen.getByRole('status')).toHaveTextContent('Performing SSH handshake...');
    act(() => useLocaleStore.setState({ locale: 'zh-CN' }));
  });

  it('主机身份确认卡在终端区域内联呈现 endpoint、算法与指纹，并提供接受并保存/仅本次接受/拒绝', async () => {
    const user = userEvent.setup();
    const accept = vi.fn();
    const save = vi.fn();
    const reject = vi.fn();
    const challenge: HostIdentityChallenge = {
      challengeId: 'challenge-1', sessionId: 'session-1', host: '10.0.0.8', port: 2222,
      keyAlgorithm: 'ssh-ed25519', fingerprint: 'SHA256:ungWv48Bz+pBQUDeXa4iI7ADYaOWF3qctBD', timestamp: 1_710_000_000_000,
    };
    render(<TerminalPane sessions={[makeSession()]} activeView="session-1"
      connections={new Map([['session-1', { phase: ConnectionPhase.VerifyingHostKey, error: null }]])}
      challenges={new Map([['session-1', challenge]])}
      onInput={vi.fn()} onResize={vi.fn()} onCreateHost={vi.fn()} onCloseTab={vi.fn()}
      onSaveIdentity={save} saveErrors={new Map()} onAcceptIdentity={accept} onRejectIdentity={reject} />);
    const card = screen.getByTestId('host-identity-card');
    expect(card).toHaveTextContent('10.0.0.8:2222');
    expect(card).toHaveTextContent('ssh-ed25519');
    expect(card).toHaveTextContent('SHA256:ungWv48Bz+pBQUDeXa4iI7ADYaOWF3qctBD');
    // 等待确认期间展示主机身份验证阶段；确认卡存在时不显示连接覆盖层；xterm 仍不可交互
    expect(card).toHaveTextContent('正在验证主机身份...');
    expect(screen.queryByRole('status')).toBeNull();
    expect(screen.getByTestId('xterm')).toHaveAttribute('data-interactive', 'false');
    await user.click(screen.getByRole('button', { name: '接受并保存' }));
    expect(save).toHaveBeenCalledWith('session-1');
    await user.click(screen.getByRole('button', { name: '仅本次接受' }));
    expect(accept).toHaveBeenCalledWith('session-1');
    await user.click(screen.getByRole('button', { name: '拒绝' }));
    expect(reject).toHaveBeenCalledWith('session-1');
  });

  it('保存失败时确认卡保持未决并展示结构化错误，用户仍可改选仅本次接受或拒绝', async () => {
    const user = userEvent.setup();
    const accept = vi.fn();
    const reject = vi.fn();
    const challenge: HostIdentityChallenge = {
      challengeId: 'challenge-1', sessionId: 'session-1', host: '10.0.0.8', port: 22,
      keyAlgorithm: 'ssh-ed25519', fingerprint: 'SHA256:abc', timestamp: 1_710_000_000_000,
    };
    render(<TerminalPane sessions={[makeSession()]} activeView="session-1"
      connections={new Map([['session-1', { phase: ConnectionPhase.VerifyingHostKey, error: null }]])}
      challenges={new Map([['session-1', challenge]])}
      saveErrors={new Map([['session-1', { code: 'HostKeySaveFailed', detail: 'write denied' }]])}
      onInput={vi.fn()} onResize={vi.fn()} onCreateHost={vi.fn()} onCloseTab={vi.fn()}
      onSaveIdentity={vi.fn()} onAcceptIdentity={accept} onRejectIdentity={reject} />);
    const card = screen.getByTestId('host-identity-card');
    const error = screen.getByTestId('host-identity-save-error');
    expect(card).toContainElement(error);
    expect(error).toHaveTextContent('保存信任记录失败');
    expect(error).toHaveTextContent('write denied');
    // 三个操作仍可用：重试保存、改选仅本次接受或拒绝
    expect(screen.getByRole('button', { name: '接受并保存' })).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: '仅本次接受' }));
    expect(accept).toHaveBeenCalledWith('session-1');
    await user.click(screen.getByRole('button', { name: '拒绝' }));
    expect(reject).toHaveBeenCalledWith('session-1');
  });

  it('非验证阶段的连接投影不向确认卡注入阶段文案', async () => {
    const challenge: HostIdentityChallenge = {
      challengeId: 'challenge-1', sessionId: 'session-1', host: '10.0.0.8', port: 22,
      keyAlgorithm: 'ssh-ed25519', fingerprint: 'SHA256:abc', timestamp: 1_710_000_000_000,
    };
    render(<TerminalPane sessions={[makeSession()]} activeView="session-1"
      connections={new Map([['session-1', { phase: ConnectionPhase.Authenticating, error: null }]])}
      challenges={new Map([['session-1', challenge]])}
      onInput={vi.fn()} onResize={vi.fn()} onCreateHost={vi.fn()} onCloseTab={vi.fn()}
      onSaveIdentity={vi.fn()} saveErrors={new Map()} onAcceptIdentity={vi.fn()} onRejectIdentity={vi.fn()} />);
    expect(screen.getByTestId('host-identity-card')).not.toHaveTextContent('正在验证主机身份...');
  });

  it('英文环境下主机身份确认卡使用英文文案', async () => {
    const user = userEvent.setup();
    act(() => useLocaleStore.setState({ locale: 'en-US' }));
    const challenge: HostIdentityChallenge = {
      challengeId: 'challenge-1', sessionId: 'session-1', host: '10.0.0.8', port: 22,
      keyAlgorithm: 'ssh-ed25519', fingerprint: 'SHA256:abc', timestamp: 1_710_000_000_000,
    };
    render(<HostIdentityCard challenge={challenge} onAcceptAndSave={vi.fn()} saveError={null} onAccept={vi.fn()} onReject={vi.fn()} />);
    expect(screen.getByText('Cannot verify host identity')).toBeInTheDocument();
    expect(screen.getByText('Host address')).toBeInTheDocument();
    expect(screen.getByText('SHA-256 fingerprint')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Accept & Save' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Accept Once' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Reject' })).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: 'Accept Once' }));
    act(() => useLocaleStore.setState({ locale: 'zh-CN' }));
  });

  it('主机身份变更卡展示已保存与呈现的算法/指纹，提供仅本次接受/替换记录/拒绝', async () => {
    const user = userEvent.setup();
    const accept = vi.fn();
    const save = vi.fn();
    const reject = vi.fn();
    const challenge: HostIdentityChallenge = {
      challengeId: 'challenge-changed', sessionId: 'session-1', host: '10.0.0.8', port: 22,
      kind: 'Changed',
      keyAlgorithm: 'ssh-rsa', fingerprint: 'SHA256:newfp',
      storedAlgorithm: 'ssh-ed25519', storedFingerprint: 'SHA256:oldfp',
      timestamp: 1_710_000_000_000,
    };
    render(<HostIdentityCard challenge={challenge} onAcceptAndSave={save} saveError={null} onAccept={accept} onReject={reject} />);
    const card = screen.getByTestId('host-identity-card');
    expect(card).toHaveTextContent('主机身份已变更');
    expect(card).toHaveTextContent('10.0.0.8:22');
    // 内联卡片清晰展示已保存旧记录与服务器呈现的算法/指纹
    const stored = within(card).getByTestId('host-identity-stored');
    expect(stored).toHaveTextContent('已保存算法');
    expect(stored).toHaveTextContent('ssh-ed25519');
    expect(stored).toHaveTextContent('已保存 SHA-256 指纹');
    expect(stored).toHaveTextContent('SHA256:oldfp');
    const presented = within(card).getByTestId('host-identity-presented');
    expect(presented).toHaveTextContent('呈现算法');
    expect(presented).toHaveTextContent('ssh-rsa');
    expect(presented).toHaveTextContent('呈现 SHA-256 指纹');
    expect(presented).toHaveTextContent('SHA256:newfp');
    // Changed 不提供一次性"接受并保存"，提供 仅本次接受/替换记录/拒绝
    expect(screen.queryByTestId('host-identity-save')).toBeNull();
    expect(screen.getByTestId('host-identity-replace')).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: '仅本次接受' }));
    expect(accept).toHaveBeenCalledTimes(1);
    await user.click(screen.getByRole('button', { name: '拒绝' }));
    expect(reject).toHaveBeenCalledTimes(1);
    expect(save).not.toHaveBeenCalled();
  });

  it('替换记录必须经过第二次内联确认：取消不回调，确认替换才回调；challenge 更换重置确认', async () => {
    const user = userEvent.setup();
    const save = vi.fn();
    const makeChallenge = (challengeId: string): HostIdentityChallenge => ({
      challengeId, sessionId: 'session-1', host: '10.0.0.8', port: 22,
      kind: 'Changed',
      keyAlgorithm: 'ssh-rsa', fingerprint: 'SHA256:newfp',
      storedAlgorithm: 'ssh-ed25519', storedFingerprint: 'SHA256:oldfp',
      timestamp: 1_710_000_000_000,
    });
    const { rerender } = render(<HostIdentityCard challenge={makeChallenge('challenge-1')}
      onAcceptAndSave={save} saveError={null} onAccept={vi.fn()} onReject={vi.fn()} />);

    // 第一次点击"替换记录"只进入二次确认，不触发替换
    await user.click(screen.getByTestId('host-identity-replace'));
    expect(screen.getByTestId('host-identity-replace-confirm')).toHaveTextContent('确认替换长期信任记录？');
    expect(save).not.toHaveBeenCalled();
    // 取消：退回主操作，仍不触发替换
    await user.click(screen.getByTestId('host-identity-replace-cancel'));
    expect(screen.queryByTestId('host-identity-replace-confirm')).toBeNull();
    expect(save).not.toHaveBeenCalled();
    // 第二次进入确认并点击"确认替换"才真正替换
    await user.click(screen.getByTestId('host-identity-replace'));
    await user.click(screen.getByTestId('host-identity-replace-confirm-btn'));
    expect(save).toHaveBeenCalledTimes(1);

    // 服务端再次换 key（新 challenge 事件）：确认状态重置，不残留二次确认
    rerender(<HostIdentityCard challenge={makeChallenge('challenge-2')}
      onAcceptAndSave={save} saveError={null} onAccept={vi.fn()} onReject={vi.fn()} />);
    expect(screen.queryByTestId('host-identity-replace-confirm')).toBeNull();
  });

  it('替换失败时确认卡保持未决并展示替换失败文案，可改选仅本次接受或拒绝', async () => {
    const user = userEvent.setup();
    const accept = vi.fn();
    const reject = vi.fn();
    const challenge: HostIdentityChallenge = {
      challengeId: 'challenge-replace-fail', sessionId: 'session-1', host: '10.0.0.8', port: 22,
      kind: 'Changed',
      keyAlgorithm: 'ssh-rsa', fingerprint: 'SHA256:newfp',
      storedAlgorithm: 'ssh-ed25519', storedFingerprint: 'SHA256:oldfp',
      timestamp: 1_710_000_000_000,
    };
    render(<HostIdentityCard challenge={challenge} onAcceptAndSave={vi.fn()}
      saveError={{ code: 'HostKeySaveFailed', detail: 'write denied' }} onAccept={accept} onReject={reject} />);
    const card = screen.getByTestId('host-identity-card');
    const error = screen.getByTestId('host-identity-save-error');
    expect(card).toContainElement(error);
    expect(error).toHaveTextContent('替换信任记录失败');
    expect(error).toHaveTextContent('write denied');
    // 替换失败后仍可改选仅本次接受或拒绝
    await user.click(screen.getByRole('button', { name: '仅本次接受' }));
    expect(accept).toHaveBeenCalledTimes(1);
    await user.click(screen.getByRole('button', { name: '拒绝' }));
    expect(reject).toHaveBeenCalledTimes(1);
  });

  it('英文环境下连接阶段与失败操作使用英文文案', async () => {
    const user = userEvent.setup();
    act(() => useLocaleStore.setState({ locale: 'en-US' }));
    const { rerender } = render(<TerminalPane sessions={[makeSession()]} activeView="session-1"
      connections={new Map([['session-1', { phase: ConnectionPhase.SshHandshake, error: null }]])}
      challenges={new Map()} onInput={vi.fn()} onResize={vi.fn()} onCreateHost={vi.fn()} onCloseTab={vi.fn()} onSaveIdentity={vi.fn()} saveErrors={new Map()} onAcceptIdentity={vi.fn()} onRejectIdentity={vi.fn()} />);
    expect(screen.getByRole('status')).toHaveTextContent('Performing SSH handshake...');
    rerender(<TerminalPane sessions={[makeSession({ status: SessionStatus.Timeout })]} activeView="session-1"
      connections={new Map([['session-1', { phase: null, error: null }]])}
      challenges={new Map()} onInput={vi.fn()} onResize={vi.fn()} onCreateHost={vi.fn()} onCloseTab={vi.fn()} onSaveIdentity={vi.fn()} saveErrors={new Map()} onAcceptIdentity={vi.fn()} onRejectIdentity={vi.fn()} />);
    expect(screen.getByRole('alert')).toHaveTextContent('Connection timed out');
    await user.click(screen.getByRole('button', { name: 'Close Tab' }));
    act(() => useLocaleStore.setState({ locale: 'zh-CN' }));
  });

  it('文件行与传输队列使用 lucide 图标而非 emoji', () => {
    const state = { currentPath: '/', entries: [makeRemoteEntry(), makeRemoteDir()], selectedPaths: new Set<string>(), loading: false, error: null, tasks: new Map(), taskActionErrors: new Map(), dirRequestSeq: 0 };
    const { container } = render(<FileExplorer state={state} onNavigate={vi.fn()} onSelect={vi.fn()} onUpload={vi.fn()} onDownload={vi.fn()} />);
    const rows = container.querySelectorAll('.file-row');
    expect(rows[0].querySelector('svg')).not.toBeNull();
    expect(rows[1].querySelector('svg')).not.toBeNull();
    const tasks = new Map([[makeTransferTask().taskId, makeTransferTask()]]);
    render(<TransferQueue tasks={tasks} actionErrors={new Map()} onCancel={vi.fn()} onRetry={vi.fn()} onOverwrite={vi.fn()} onClearTerminal={vi.fn()} />);
    expect(container.querySelectorAll('svg').length).toBeGreaterThan(0);
    expect(screen.queryByText('📁')).not.toBeInTheDocument();
    expect(screen.queryByText('⬇')).not.toBeInTheDocument();
  });

  it('折叠窄条箭头朝上提示展开，展开面板箭头朝下提示收起', () => {
    const { container, rerender } = render(<ServerStatusPanel snapshot={null} collapsed onToggle={vi.fn()} />);
    expect(container.querySelector('.monitor-strip-chevron')).toHaveClass('lucide-chevron-up');
    rerender(<ServerStatusPanel snapshot={null} collapsed={false} onToggle={vi.fn()} />);
    expect(container.querySelector('.monitor-collapse-btn svg')).toHaveClass('lucide-chevron-down');
  });

  it('服务器状态正确显示占位、指标和磁盘容量', () => {
    const { rerender } = render(<ServerStatusPanel snapshot={null} collapsed={false} onToggle={vi.fn()} />);
    expect(screen.getByText('未连接')).toBeInTheDocument();
    rerender(<ServerStatusPanel snapshot={makeSnapshot()} collapsed={false} onToggle={vi.fn()} />);
    expect(screen.getByText('21.5%')).toBeInTheDocument();
    expect(screen.getByText(/剩余 300.0 GB \/ 总量 500.0 GB/)).toBeInTheDocument();
    expect(screen.queryByText(/^Updated:/)).not.toBeInTheDocument();
  });

  it('指标缺失时显示未知（--）而非伪造 0%', () => {
    render(<ServerStatusPanel snapshot={makeSnapshot({
      cpuUsage: null,
      memoryUsage: null,
      diskUsage: null,
      diskAvailableBytes: null,
      diskTotalBytes: null,
    })} collapsed={false} onToggle={vi.fn()} />);
    expect(screen.getAllByText('--')).toHaveLength(3);
    expect(screen.queryByText('0.0%')).not.toBeInTheDocument();
  });

  it('服务器状态显示默认网卡速率，并区分无网卡和网络不可用', () => {
    const { rerender } = render(<ServerStatusPanel snapshot={makeSnapshot({ network: {
      available: true,
      interfaces: [{ name: 'eth0', receiveBytesPerSecond: 1536, transmitBytesPerSecond: 0 }],
    } })} collapsed={false} onToggle={vi.fn()} />);
    expect(screen.getByText('下行 · eth0')).toBeInTheDocument();
    expect(screen.getByText('1.5 KB/s')).toBeInTheDocument();
    expect(screen.getByText('0 B/s')).toBeInTheDocument();

    rerender(<ServerStatusPanel snapshot={makeSnapshot({ network: {
      available: true,
      interfaces: [{ name: 'eth0', receiveBytesPerSecond: null, transmitBytesPerSecond: null }],
    } })} collapsed={false} onToggle={vi.fn()} />);
    expect(screen.getAllByText('--')).toHaveLength(2);

    rerender(<ServerStatusPanel snapshot={makeSnapshot({ network: { available: true, interfaces: [] } })} collapsed={false} onToggle={vi.fn()} />);
    expect(screen.getByText('无可用网卡')).toBeInTheDocument();

    rerender(<ServerStatusPanel snapshot={makeSnapshot({ network: { available: false, interfaces: [] } })} collapsed={false} onToggle={vi.fn()} />);
    expect(screen.getByText('网络数据不可用')).toBeInTheDocument();
  });

  it('服务器状态下拉框展示全部网卡并切换当前速率', () => {
    const onInterfaceChange = vi.fn();
    render(<ServerStatusPanel snapshot={makeSnapshot({ network: {
      available: true,
      interfaces: [
        { name: 'eth0', receiveBytesPerSecond: 1024, transmitBytesPerSecond: 512 },
        { name: 'eth1', receiveBytesPerSecond: 2048, transmitBytesPerSecond: 1024 },
      ],
    } })} selectedInterfaceName="eth1" onInterfaceChange={onInterfaceChange} collapsed={false} onToggle={vi.fn()} />);
    const selector = screen.getByLabelText('网卡接口') as HTMLSelectElement;
    expect(selector).toHaveValue('eth1');
    expect([...selector.options].map((option) => option.value)).toEqual(['eth0', 'eth1']);
    expect(screen.getByText('2.0 KB/s')).toBeInTheDocument();
    fireEvent.change(selector, { target: { value: 'eth0' } });
    expect(onInterfaceChange).toHaveBeenCalledWith('eth0');
  });

  it('服务器状态使用可访问 SVG 展示双向一分钟趋势和图例', () => {
    render(<ServerStatusPanel snapshot={makeSnapshot({ network: {
      available: true,
      interfaces: [{ name: 'eth0', receiveBytesPerSecond: 2048, transmitBytesPerSecond: 1024 }],
    } })} selectedInterfaceName="eth0" trendSamples={[
      { timestamp: 1_000, receiveBytesPerSecond: 1024, transmitBytesPerSecond: 512 },
      { timestamp: 2_000, receiveBytesPerSecond: null, transmitBytesPerSecond: null },
      { timestamp: 3_000, receiveBytesPerSecond: 2048, transmitBytesPerSecond: 1024 },
    ]} collapsed={false} onToggle={vi.fn()} />);
    expect(screen.getByRole('img', { name: '最近一分钟网卡速率趋势' })).toBeInTheDocument();
    expect(screen.getByText('下行趋势')).toBeInTheDocument();
    expect(screen.getByText('上行趋势')).toBeInTheDocument();
    expect(screen.getByText('60 秒前')).toBeInTheDocument();
    expect(screen.getByText('现在')).toBeInTheDocument();
    expect(screen.getByText('2.0 KB/s')).toBeInTheDocument();
  });

  it('监视条折叠态显示状态点，点击请求展开', async () => {
    const user = userEvent.setup();
    const onToggle = vi.fn();
    const { rerender } = render(<ServerStatusPanel snapshot={makeSnapshot()} collapsed onToggle={onToggle} />);
    expect(screen.getByTestId('monitor-strip')).toBeInTheDocument();
    expect(screen.queryByText('21.5%')).not.toBeInTheDocument();
    await user.click(screen.getByTestId('monitor-strip'));
    expect(onToggle).toHaveBeenCalledOnce();
    rerender(<ServerStatusPanel snapshot={makeSnapshot()} collapsed={false} onToggle={onToggle} />);
    expect(screen.getByText('21.5%')).toBeInTheDocument();
  });

  it('文件浏览器目录优先，并区分选择、导航与下载', async () => {
    const user = userEvent.setup();
    const navigate = vi.fn();
    const select = vi.fn();
    const download = vi.fn();
    const state = { currentPath: '/var/log', entries: [makeRemoteEntry(), makeRemoteDir()], selectedPaths: new Set<string>(), loading: false, error: null, tasks: new Map(), taskActionErrors: new Map(), dirRequestSeq: 0 };
    render(<FileExplorer state={state} onNavigate={navigate} onSelect={select} onUpload={vi.fn()} onDownload={download} />);
    expect(screen.getAllByTestId('file-row')[0]).toHaveTextContent('nginx');
    await user.click(screen.getByText('syslog'));
    fireEvent.doubleClick(screen.getByText('nginx'));
    fireEvent.doubleClick(screen.getByText('syslog'));
    expect(select).toHaveBeenCalledWith('/var/log/syslog');
    expect(navigate).toHaveBeenCalledWith('/var/log/nginx');
    expect(download).toHaveBeenCalledWith(['/var/log/syslog']);
  });

  it('文件浏览器刷新当前目录', async () => {
    const user = userEvent.setup();
    const navigate = vi.fn();
    const state = { currentPath: '/var/log', entries: [], selectedPaths: new Set<string>(), loading: false, error: null, tasks: new Map(), taskActionErrors: new Map(), dirRequestSeq: 0 };
    render(<FileExplorer state={state} onNavigate={navigate} onSelect={vi.fn()} onUpload={vi.fn()} onDownload={vi.fn()} />);

    await user.click(screen.getByRole('button', { name: '刷新' }));

    expect(navigate).toHaveBeenCalledWith('/var/log');
  });

  it('文件浏览器加载目录时禁用刷新', () => {
    const state = { currentPath: '/', entries: [], selectedPaths: new Set<string>(), loading: true, error: null, tasks: new Map(), taskActionErrors: new Map(), dirRequestSeq: 0 };
    render(<FileExplorer state={state} onNavigate={vi.fn()} onSelect={vi.fn()} onUpload={vi.fn()} onDownload={vi.fn()} />);

    expect(screen.getByRole('button', { name: '刷新' })).toBeDisabled();
  });

  it('文件浏览器右键文件可下载该文件', async () => {
    const user = userEvent.setup();
    const download = vi.fn();
    const state = { currentPath: '/var/log', entries: [makeRemoteEntry()], selectedPaths: new Set<string>(), loading: false, error: null, tasks: new Map(), taskActionErrors: new Map(), dirRequestSeq: 0 };
    render(<FileExplorer state={state} onNavigate={vi.fn()} onSelect={vi.fn()} onUpload={vi.fn()} onDownload={download} />);

    fireEvent.contextMenu(screen.getByText('syslog'));
    await user.click(await screen.findByRole('menuitem', { name: '下载' }));

    expect(download).toHaveBeenCalledWith(['/var/log/syslog']);
  });

  it('文件浏览器右键空白区域可刷新当前目录', async () => {
    const user = userEvent.setup();
    const navigate = vi.fn();
    const state = { currentPath: '/var/log', entries: [], selectedPaths: new Set<string>(), loading: false, error: null, tasks: new Map(), taskActionErrors: new Map(), dirRequestSeq: 0 };
    const { container } = render(<FileExplorer state={state} onNavigate={navigate} onSelect={vi.fn()} onUpload={vi.fn()} onDownload={vi.fn()} />);

    fireEvent.contextMenu(container.querySelector('.file-explorer')!);
    await user.click(await screen.findByRole('menuitem', { name: '刷新' }));

    expect(navigate).toHaveBeenCalledWith('/var/log');
  });

  it('文件浏览器显示 loading、error 与空目录状态', () => {
    const props = { onNavigate: vi.fn(), onSelect: vi.fn(), onUpload: vi.fn(), onDownload: vi.fn() };
    const base = { currentPath: '/', entries: [], selectedPaths: new Set<string>(), loading: true, error: null, tasks: new Map(), taskActionErrors: new Map(), dirRequestSeq: 0 };
    const { rerender } = render(<FileExplorer state={base} {...props} />);
    expect(screen.getByText('加载中...')).toBeInTheDocument();
    rerender(<FileExplorer state={{ ...base, loading: false, error: { code: 'Unknown', detail: 'denied' } }} {...props} />);
    expect(screen.getByText(/denied/)).toBeInTheDocument();
    rerender(<FileExplorer state={{ ...base, loading: false }} {...props} />);
    expect(screen.getByText('空目录')).toBeInTheDocument();
  });

  it('传输队列显示进度、失败原因并支持取消和重试', async () => {
    const user = userEvent.setup();
    const cancel = vi.fn();
    const retry = vi.fn();
    const running = makeTransferTask({ transferredBytes: 25600, speedBps: 1024, status: 'Running' });
    const failed = makeTransferTask({ taskId: 'task-2', status: 'Failed', error: { code: 'SftpTransferError', detail: 'network' } });
    render(<TransferQueue tasks={new Map([[running.taskId, running], [failed.taskId, failed]])} actionErrors={new Map()} onCancel={cancel} onRetry={retry} onOverwrite={vi.fn()} onClearTerminal={vi.fn()} />);
    expect(screen.getByText('50%')).toBeInTheDocument();
    expect(screen.getByText(/network/)).toBeInTheDocument();
    await user.click(screen.getByTestId('cancel-btn'));
    await user.click(screen.getByTestId('retry-btn'));
    expect(cancel).toHaveBeenCalledWith(running.taskId);
    expect(retry).toHaveBeenCalledWith(failed);
  });

  it('冲突失败任务行显示覆盖按钮，点击仅回调该任务', async () => {
    const user = userEvent.setup();
    const overwrite = vi.fn();
    const conflict = makeTransferTask({
      taskId: 'task-conflict', status: 'Failed',
      error: { code: 'SftpTargetExists', detail: '/Users/user/Downloads/syslog' },
    });
    const other = makeTransferTask({ taskId: 'task-other', status: 'Failed', error: { code: 'SftpReadError', detail: 'reset' } });
    render(<TransferQueue tasks={new Map([[conflict.taskId, conflict], [other.taskId, other]])}
      actionErrors={new Map()} onCancel={vi.fn()} onRetry={vi.fn()} onOverwrite={overwrite} onClearTerminal={vi.fn()} />);
    expect(screen.getByText(/目标文件已存在/)).toBeInTheDocument();
    expect(screen.getByTestId('overwrite-btn')).toHaveTextContent('覆盖下载');
    await user.click(screen.getByTestId('overwrite-btn'));
    expect(overwrite).toHaveBeenCalledWith(conflict);
  });

  it('上传冲突失败任务行显示覆盖按钮，点击仅回调该任务', async () => {
    const user = userEvent.setup();
    const overwrite = vi.fn();
    const conflict = makeTransferTask({
      taskId: 'task-upload-conflict', transferType: 'Upload', remotePath: '/var/log/syslog',
      fileName: 'syslog', localPath: '/tmp/syslog', status: 'Failed',
      error: { code: 'SftpTargetExists', detail: '/var/log/syslog' },
    });
    render(<TransferQueue tasks={new Map([[conflict.taskId, conflict]])}
      actionErrors={new Map()} onCancel={vi.fn()} onRetry={vi.fn()} onOverwrite={overwrite} onClearTerminal={vi.fn()} />);
    expect(screen.getByTestId('overwrite-btn')).toHaveTextContent('覆盖上传');
    await user.click(screen.getByTestId('overwrite-btn'));
    expect(overwrite).toHaveBeenCalledWith(conflict);
  });

  it('非冲突失败任务不显示覆盖按钮', () => {
    const task = makeTransferTask({ status: 'Failed', error: { code: 'SftpReadError', detail: 'reset' } });
    render(<TransferQueue tasks={new Map([[task.taskId, task]])}
      actionErrors={new Map()} onCancel={vi.fn()} onRetry={vi.fn()} onOverwrite={vi.fn()} onClearTerminal={vi.fn()} />);
    expect(screen.queryByTestId('overwrite-btn')).toBeNull();
  });

  it('取消后临时文件清理失败的错误在任务行可见', () => {
    const task = makeTransferTask({
      status: 'Cancelled',
      error: { code: 'SftpTransferError', detail: '清理临时文件失败: /tmp/.syslog.task.part (not empty)' },
    });
    render(<TransferQueue tasks={new Map([[task.taskId, task]])}
      actionErrors={new Map()} onCancel={vi.fn()} onRetry={vi.fn()} onOverwrite={vi.fn()} onClearTerminal={vi.fn()} />);
    expect(screen.getByText(/清理临时文件失败/)).toBeInTheDocument();
  });

  it('任务行渲染取消/重试操作失败的结构化错误', () => {
    const task = makeTransferTask({ status: 'Running' });
    const actionErrors = new Map([[task.taskId, { code: 'SftpTaskNotFound', detail: task.taskId }]]);
    render(<TransferQueue tasks={new Map([[task.taskId, task]])} actionErrors={actionErrors} onCancel={vi.fn()} onRetry={vi.fn()} onOverwrite={vi.fn()} onClearTerminal={vi.fn()} />);
    expect(screen.getByTestId('task-action-error')).toHaveTextContent('SFTP 任务不存在');
    expect(screen.getByTestId('task-action-error')).toHaveTextContent(task.taskId);
  });

  it('传输队列按 createdAt 最新优先展示任务', () => {
    const oldTask = makeTransferTask({ taskId: 'task-old', fileName: 'old.log', createdAt: 1_000 });
    const newTask = makeTransferTask({ taskId: 'task-new', fileName: 'new.log', createdAt: 2_000 });
    const { container } = render(<TransferQueue tasks={new Map([[oldTask.taskId, oldTask], [newTask.taskId, newTask]])}
      actionErrors={new Map()} onCancel={vi.fn()} onRetry={vi.fn()} onOverwrite={vi.fn()} onClearTerminal={vi.fn()} />);
    const names = [...container.querySelectorAll('.task-name')].map((node) => node.textContent);
    expect(names).toEqual(['new.log', 'old.log']);
  });

  it('传输队列存在终态任务时显示清除按钮并回调，仅活动任务时不显示', async () => {
    const user = userEvent.setup();
    const onClearTerminal = vi.fn();
    const done = makeTransferTask({ taskId: 'task-done', status: 'Done' });
    const running = makeTransferTask({ taskId: 'task-running', status: 'Running' });
    const props = { actionErrors: new Map(), onCancel: vi.fn(), onRetry: vi.fn(), onOverwrite: vi.fn() };
    const { rerender } = render(<TransferQueue tasks={new Map([[done.taskId, done], [running.taskId, running]])}
      {...props} onClearTerminal={onClearTerminal} />);
    await user.click(screen.getByTestId('clear-terminal-btn'));
    expect(onClearTerminal).toHaveBeenCalledOnce();
    rerender(<TransferQueue tasks={new Map([[running.taskId, running]])} {...props} onClearTerminal={onClearTerminal} />);
    expect(screen.queryByTestId('clear-terminal-btn')).not.toBeInTheDocument();
  });

  it('SFTP 面板在浏览器和队列间切换并保留占位', async () => {
    const user = userEvent.setup();
    const handlers = { onNavigate: vi.fn(), onSelect: vi.fn(), onUpload: vi.fn(), onDownload: vi.fn(), onCancel: vi.fn(), onRetry: vi.fn(), onOverwrite: vi.fn(), onClearTerminal: vi.fn() };
    const { rerender } = render(<SftpPanel sessionId="session-1" state={null} {...handlers} />);
    expect(screen.getByText('请选择会话')).toBeInTheDocument();
    const state = { currentPath: '/', entries: [], selectedPaths: new Set<string>(), loading: false, error: null, tasks: new Map(), taskActionErrors: new Map(), dirRequestSeq: 0 };
    rerender(<SftpPanel sessionId="session-1" state={state} {...handlers} />);
    await user.click(screen.getByTestId('tab-queue'));
    expect(screen.getByText('暂无传输任务')).toBeInTheDocument();
    expect(screen.getByTestId('sftp-resizer')).toHaveAttribute('aria-orientation', 'horizontal');
  });

  it('SFTP 高度拖动阻止文本选中：阻止默认行为并仅拖动期间加禁选类', () => {
    const handlers = { onNavigate: vi.fn(), onSelect: vi.fn(), onUpload: vi.fn(), onDownload: vi.fn(), onCancel: vi.fn(), onRetry: vi.fn(), onOverwrite: vi.fn(), onClearTerminal: vi.fn() };
    render(<SftpPanel sessionId="session-1" state={null} {...handlers} />);
    const resizer = screen.getByTestId('sftp-resizer');
    const start = new Event('pointerdown', { bubbles: true, cancelable: true });
    const preventDefault = vi.spyOn(start, 'preventDefault');
    Object.defineProperty(start, 'clientY', { value: 400 });
    fireEvent(resizer, start);
    expect(preventDefault).toHaveBeenCalled();
    expect(document.body.classList.contains('sftp-resizing')).toBe(true);
    fireEvent(window, new Event('pointerup'));
    expect(document.body.classList.contains('sftp-resizing')).toBe(false);
  });

  it('保存失败错误显示在表单内', () => {
    render(<HostEditorDialog open editingHost={null} groups={[]} onClose={vi.fn()} onSave={vi.fn()}
      saveError={{ code: 'SecureStoreError', detail: 'The name org.freedesktop.secrets was not provided by any .service files' }} />);
    expect(screen.getByTestId('host-editor-save-error'))
      .toHaveTextContent('安全存储错误: The name org.freedesktop.secrets was not provided by any .service files');
  });

  it('主机表单编辑时不回填密码，并按认证方式清理字段', async () => {
    const user = userEvent.setup();
    const save = vi.fn();
    render(<HostEditorDialog open editingHost={makeHost()} groups={[]} onClose={vi.fn()} onSave={save} />);
    expect(screen.getByDisplayValue('prod')).toBeInTheDocument();
    const password = screen.getByPlaceholderText('留空则保持原密码不变');
    expect(password).toHaveValue('');
    await user.type(password, 'new-secret');
    await user.click(screen.getByText('保存连接'));
    expect(save).toHaveBeenCalledWith(expect.objectContaining({ authType: AuthType.Password, password: 'new-secret', privateKeyPath: undefined, group: 'production' }));
  });

  it('主机表单可输入新分组名并随保存提交', async () => {
    const user = userEvent.setup();
    const save = vi.fn();
    render(<HostEditorDialog open editingHost={null} groups={['production']} onClose={vi.fn()} onSave={save} />);
    await user.type(screen.getByRole('combobox', { name: '分组' }), 'blue-team');
    await user.click(screen.getByText('保存连接'));
    expect(save).toHaveBeenCalledWith(expect.objectContaining({ group: 'blue-team' }));
  });

  it('主机表单可选择已有分组', async () => {
    const user = userEvent.setup();
    const save = vi.fn();
    render(<HostEditorDialog open editingHost={null} groups={['production', 'staging']} onClose={vi.fn()} onSave={save} />);
    await user.click(screen.getByRole('combobox', { name: '分组' }));
    await user.click(await screen.findByText('staging'));
    await user.click(screen.getByText('保存连接'));
    expect(save).toHaveBeenCalledWith(expect.objectContaining({ group: 'staging' }));
  });

  it('私钥模式通过系统选择器选择私钥路径并回填保存', async () => {
    const user = userEvent.setup();
    const save = vi.fn();
    vi.mocked(openFileDialog).mockResolvedValueOnce('/Users/me/.ssh/id_ed25519');
    render(<HostEditorDialog open editingHost={makeHost()} groups={[]} onClose={vi.fn()} onSave={save} />);
    expect(screen.queryByRole('button', { name: /浏览/ })).not.toBeInTheDocument();
    await user.click(screen.getByRole('combobox', { name: '认证方式' }));
    await user.click(await screen.findByText('私钥'));
    await user.click(screen.getByRole('button', { name: /浏览/ }));
    expect(await screen.findByDisplayValue('/Users/me/.ssh/id_ed25519')).toBeInTheDocument();
    await user.click(screen.getByText('保存连接'));
    expect(save).toHaveBeenCalledWith(expect.objectContaining({ authType: AuthType.PrivateKey, privateKeyPath: '/Users/me/.ssh/id_ed25519' }));
    expect(openFileDialog).toHaveBeenCalledWith({ multiple: false, directory: false, title: '选择私钥文件' });
  });

  it('取消选择器时保持私钥路径为空并拦截保存', async () => {
    const user = userEvent.setup();
    const save = vi.fn();
    vi.mocked(openFileDialog).mockResolvedValueOnce(null);
    render(<HostEditorDialog open editingHost={makeHost({ authType: AuthType.PrivateKey })} groups={[]} onClose={vi.fn()} onSave={save} />);
    await user.click(screen.getByRole('button', { name: /浏览/ }));
    expect(screen.getByPlaceholderText('点击浏览选择私钥文件')).toHaveValue('');
    fireEvent.click(screen.getByText('保存连接'));
    expect(save).not.toHaveBeenCalled();
  });

  it('私钥路径为空时禁用保存', () => {
    render(<HostEditorDialog open editingHost={makeHost({ authType: AuthType.PrivateKey })} groups={[]} onClose={vi.fn()} onSave={vi.fn()} />);
    expect(screen.queryByText('请先选择私钥文件')).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: /保存连接/ })).toBeDisabled();
  });
});
