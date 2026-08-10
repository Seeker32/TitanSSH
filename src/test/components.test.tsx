import { open as openFileDialog } from '@tauri-apps/plugin-dialog';
import { fireEvent, render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import FileExplorer from '@/components/sftp/FileExplorer';
import HostEditorDialog from '@/components/host/HostEditorDialog';
import HostListSidebar from '@/components/host/HostListSidebar';
import { groupHosts } from '@/stores/host';
import ServerStatusPanel from '@/components/status/ServerStatusPanel';
import SftpPanel from '@/components/sftp/SftpPanel';
import TerminalPane from '@/components/terminal/TerminalPane';
import TerminalTabs from '@/components/terminal/TerminalTabs';
import TransferQueue from '@/components/sftp/TransferQueue';
import { AuthType } from '@/types/host';
import { SessionStatus } from '@/types/session';
import { makeHost, makeRemoteDir, makeRemoteEntry, makeSession, makeSnapshot, makeTransferTask } from './fixtures';

vi.mock('@/components/terminal/XtermView', () => ({
  default: ({ sessionId, active }: { sessionId: string; active: boolean }) => <div data-testid="xterm" data-session={sessionId} hidden={!active} />,
}));

describe('React components', () => {
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
      onInput={vi.fn()} onResize={vi.fn()} />);
    const terminals = screen.getAllByTestId('xterm');
    expect(terminals).toHaveLength(2);
    expect(terminals[0]).not.toBeVisible();
    expect(terminals[1]).toBeVisible();
  });

  it('无会话时终端面板显示空态页并可新建主机', () => {
    const onCreateHost = vi.fn();
    render(<TerminalPane sessions={[]} activeView={null} onInput={vi.fn()} onResize={vi.fn()} onCreateHost={onCreateHost} />);
    expect(screen.getByText(/选择左侧主机/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: '新建主机' }));
    expect(onCreateHost).toHaveBeenCalledOnce();
  });

  it('文件行与传输队列使用 lucide 图标而非 emoji', () => {
    const state = { currentPath: '/', entries: [makeRemoteEntry(), makeRemoteDir()], selectedPaths: new Set<string>(), loading: false, error: null, tasks: new Map() };
    const { container } = render(<FileExplorer state={state} onNavigate={vi.fn()} onSelect={vi.fn()} onUpload={vi.fn()} onDownload={vi.fn()} />);
    const rows = container.querySelectorAll('.file-row');
    expect(rows[0].querySelector('svg')).not.toBeNull();
    expect(rows[1].querySelector('svg')).not.toBeNull();
    const tasks = new Map([[makeTransferTask().taskId, makeTransferTask()]]);
    render(<TransferQueue tasks={tasks} onCancel={vi.fn()} onRetry={vi.fn()} />);
    expect(container.querySelectorAll('svg').length).toBeGreaterThan(0);
    expect(screen.queryByText('📁')).not.toBeInTheDocument();
    expect(screen.queryByText('⬇')).not.toBeInTheDocument();
  });

  it('服务器状态正确显示占位、指标和磁盘容量', () => {
    const { rerender } = render(<ServerStatusPanel snapshot={null} collapsed={false} onToggle={vi.fn()} />);
    expect(screen.getByText('未连接')).toBeInTheDocument();
    rerender(<ServerStatusPanel snapshot={makeSnapshot()} collapsed={false} onToggle={vi.fn()} />);
    expect(screen.getByText('21.5%')).toBeInTheDocument();
    expect(screen.getByText(/剩余 300.0 GB \/ 总量 500.0 GB/)).toBeInTheDocument();
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
    const state = { currentPath: '/var/log', entries: [makeRemoteEntry(), makeRemoteDir()], selectedPaths: new Set<string>(), loading: false, error: null, tasks: new Map() };
    render(<FileExplorer state={state} onNavigate={navigate} onSelect={select} onUpload={vi.fn()} onDownload={download} />);
    expect(screen.getAllByTestId('file-row')[0]).toHaveTextContent('nginx');
    await user.click(screen.getByText('syslog'));
    fireEvent.doubleClick(screen.getByText('nginx'));
    fireEvent.doubleClick(screen.getByText('syslog'));
    expect(select).toHaveBeenCalledWith('/var/log/syslog');
    expect(navigate).toHaveBeenCalledWith('/var/log/nginx');
    expect(download).toHaveBeenCalledWith(['/var/log/syslog']);
  });

  it('文件浏览器显示 loading、error 与空目录状态', () => {
    const props = { onNavigate: vi.fn(), onSelect: vi.fn(), onUpload: vi.fn(), onDownload: vi.fn() };
    const base = { currentPath: '/', entries: [], selectedPaths: new Set<string>(), loading: true, error: null, tasks: new Map() };
    const { rerender } = render(<FileExplorer state={base} {...props} />);
    expect(screen.getByText('加载中...')).toBeInTheDocument();
    rerender(<FileExplorer state={{ ...base, loading: false, error: 'denied' }} {...props} />);
    expect(screen.getByText('denied')).toBeInTheDocument();
    rerender(<FileExplorer state={{ ...base, loading: false }} {...props} />);
    expect(screen.getByText('空目录')).toBeInTheDocument();
  });

  it('传输队列显示进度、失败原因并支持取消和重试', async () => {
    const user = userEvent.setup();
    const cancel = vi.fn();
    const retry = vi.fn();
    const running = makeTransferTask({ transferredBytes: 25600, speedBps: 1024, status: 'Running' });
    const failed = makeTransferTask({ taskId: 'task-2', status: 'Failed', errorMessage: 'network' });
    render(<TransferQueue tasks={new Map([[running.taskId, running], [failed.taskId, failed]])} onCancel={cancel} onRetry={retry} />);
    expect(screen.getByText('50%')).toBeInTheDocument();
    expect(screen.getByText('network')).toBeInTheDocument();
    await user.click(screen.getByTestId('cancel-btn'));
    await user.click(screen.getByTestId('retry-btn'));
    expect(cancel).toHaveBeenCalledWith(running.taskId);
    expect(retry).toHaveBeenCalledWith(failed);
  });

  it('SFTP 面板在浏览器和队列间切换并保留占位', async () => {
    const user = userEvent.setup();
    const handlers = { onNavigate: vi.fn(), onSelect: vi.fn(), onUpload: vi.fn(), onDownload: vi.fn(), onCancel: vi.fn(), onRetry: vi.fn() };
    const { rerender } = render(<SftpPanel sessionId="session-1" state={null} {...handlers} />);
    expect(screen.getByText('请选择会话')).toBeInTheDocument();
    const state = { currentPath: '/', entries: [], selectedPaths: new Set<string>(), loading: false, error: null, tasks: new Map() };
    rerender(<SftpPanel sessionId="session-1" state={state} {...handlers} />);
    await user.click(screen.getByTestId('tab-queue'));
    expect(screen.getByText('暂无传输任务')).toBeInTheDocument();
    expect(screen.getByTestId('sftp-resizer')).toHaveAttribute('aria-orientation', 'horizontal');
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

  it('私钥路径为空时禁用保存并显示引导文案', () => {
    render(<HostEditorDialog open editingHost={makeHost({ authType: AuthType.PrivateKey })} groups={[]} onClose={vi.fn()} onSave={vi.fn()} />);
    expect(screen.getByText('请先选择私钥文件')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /保存连接/ })).toBeDisabled();
  });
});
