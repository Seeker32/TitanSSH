import { open as openFileDialog } from '@tauri-apps/plugin-dialog';
import { fireEvent, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import FileExplorer from '@/components/sftp/FileExplorer';
import HomeQuickActions from '@/components/home/HomeQuickActions';
import HostEditorDialog from '@/components/host/HostEditorDialog';
import HostListSidebar from '@/components/host/HostListSidebar';
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
  it('首页主机卡片分别触发连接、编辑、删除和新建', async () => {
    const user = userEvent.setup();
    const handlers = { onOpen: vi.fn(), onEdit: vi.fn(), onRemove: vi.fn(), onCreate: vi.fn() };
    render(<HomeQuickActions hosts={[makeHost()]} {...handlers} />);
    await user.click(screen.getByText('root@10.0.0.8:22'));
    await user.click(screen.getByText('编辑'));
    await user.click(screen.getByText('删除'));
    await user.click(screen.getByText('+ 新建主机'));
    expect(handlers.onOpen).toHaveBeenCalledWith('host-1');
    expect(handlers.onEdit).toHaveBeenCalledWith('host-1');
    expect(handlers.onRemove).toHaveBeenCalledWith('host-1');
    expect(handlers.onCreate).toHaveBeenCalledOnce();
  });

  it('空主机列表显示引导文案', () => {
    render(<HomeQuickActions hosts={[]} onOpen={vi.fn()} onEdit={vi.fn()} onRemove={vi.fn()} onCreate={vi.fn()} />);
    expect(screen.getByText(/暂无保存的主机/)).toBeInTheDocument();
  });

  it('侧栏主机卡片渲染服务器图标，单击选中、双击连接', async () => {
    const user = userEvent.setup();
    const handlers = { onSearchChange: vi.fn(), onSelect: vi.fn(), onOpen: vi.fn(), onCreate: vi.fn() };
    render(<HostListSidebar hosts={[makeHost()]} searchQuery="" selectedHostId={null} {...handlers} />);
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
    const { rerender } = render(<HostListSidebar hosts={[makeHost()]} searchQuery="" selectedHostId={null} {...handlers} />);
    await user.type(screen.getByPlaceholderText('搜索主机…'), 'prod');
    expect(handlers.onSearchChange).toHaveBeenCalledWith('p');
    rerender(<HostListSidebar hosts={[]} searchQuery="prod" selectedHostId={null} {...handlers} />);
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

  it('终端标签保持首页并按会话状态渲染和关闭', async () => {
    const user = userEvent.setup();
    const activate = vi.fn();
    const close = vi.fn();
    render(<TerminalTabs sessions={[makeSession({ status: SessionStatus.Connected })]} activeView="session-1" onActivate={activate} onClose={close} />);
    expect(screen.getAllByRole('tab')).toHaveLength(2);
    expect(document.querySelector('.dot-connected')).toBeInTheDocument();
    await user.click(screen.getByText('首页'));
    await user.click(screen.getByLabelText('关闭 root@10.0.0.8'));
    expect(activate).toHaveBeenCalledWith('home');
    expect(close).toHaveBeenCalledWith('session-1');
  });

  it('终端面板保留每个会话实例，仅展示当前视图', () => {
    render(<TerminalPane sessions={[makeSession(), makeSession({ sessionId: 'session-2' })]} activeView="session-2" hosts={[]}
      onInput={vi.fn()} onResize={vi.fn()} onOpenHost={vi.fn()} onEditHost={vi.fn()} onRemoveHost={vi.fn()} onCreateHost={vi.fn()} />);
    const terminals = screen.getAllByTestId('xterm');
    expect(terminals).toHaveLength(2);
    expect(terminals[0]).not.toBeVisible();
    expect(terminals[1]).toBeVisible();
  });

  it('服务器状态正确显示占位、指标和磁盘容量', () => {
    const { rerender } = render(<ServerStatusPanel snapshot={null} />);
    expect(screen.getByText('未连接')).toBeInTheDocument();
    rerender(<ServerStatusPanel snapshot={makeSnapshot()} />);
    expect(screen.getByText('21.5%')).toBeInTheDocument();
    expect(screen.getByText(/剩余 300.0 GB \/ 总量 500.0 GB/)).toBeInTheDocument();
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
    render(<HostEditorDialog open editingHost={makeHost()} onClose={vi.fn()} onSave={save} />);
    expect(screen.getByDisplayValue('prod')).toBeInTheDocument();
    const password = screen.getByPlaceholderText('留空则保持原密码不变');
    expect(password).toHaveValue('');
    await user.type(password, 'new-secret');
    await user.click(screen.getByText('保存连接'));
    expect(save).toHaveBeenCalledWith(expect.objectContaining({ authType: AuthType.Password, password: 'new-secret', privateKeyPath: undefined, group: 'production' }));
  });

  it('私钥模式通过系统选择器选择私钥路径并回填保存', async () => {
    const user = userEvent.setup();
    const save = vi.fn();
    vi.mocked(openFileDialog).mockResolvedValueOnce('/Users/me/.ssh/id_ed25519');
    render(<HostEditorDialog open editingHost={makeHost()} onClose={vi.fn()} onSave={save} />);
    expect(screen.queryByRole('button', { name: /浏览/ })).not.toBeInTheDocument();
    await user.click(screen.getByRole('combobox'));
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
    render(<HostEditorDialog open editingHost={makeHost({ authType: AuthType.PrivateKey })} onClose={vi.fn()} onSave={save} />);
    await user.click(screen.getByRole('button', { name: /浏览/ }));
    expect(screen.getByPlaceholderText('点击浏览选择私钥文件')).toHaveValue('');
    fireEvent.click(screen.getByText('保存连接'));
    expect(save).not.toHaveBeenCalled();
  });

  it('私钥路径为空时禁用保存并显示引导文案', () => {
    render(<HostEditorDialog open editingHost={makeHost({ authType: AuthType.PrivateKey })} onClose={vi.fn()} onSave={vi.fn()} />);
    expect(screen.getByText('请先选择私钥文件')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /保存连接/ })).toBeDisabled();
  });
});
