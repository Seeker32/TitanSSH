import { Moon, Settings, Sun } from 'lucide-react';
import { useEffect, useMemo, useRef, useState } from 'react';
import { Typography } from 'antd';
import { open as openFileDialog, save as saveFileDialog } from '@tauri-apps/plugin-dialog';
import HostEditorDialog from '@/components/host/HostEditorDialog';
import HostListSidebar from '@/components/host/HostListSidebar';
import SftpPanel from '@/components/sftp/SftpPanel';
import ServerStatusPanel from '@/components/status/ServerStatusPanel';
import TerminalPane from '@/components/terminal/TerminalPane';
import TerminalTabs from '@/components/terminal/TerminalTabs';
import { filterHosts, useHostStore } from '@/stores/host';
import { clampSidebarWidth, MAX_SIDEBAR_WIDTH, useLayoutStore } from '@/stores/layout';
import { useMonitorStore } from '@/stores/monitor';
import { useSessionStore } from '@/stores/session';
import { useSftpStore } from '@/stores/sftp';
import { useThemeStore } from '@/stores/theme';
import type { HostConfig, SaveHostRequest } from '@/types/host';
import type { TransferTask } from '@/types/sftp';

/** 协调主机、会话、终端、监控与 SFTP 视图。 */
export default function HomePage() {
  const hosts = useHostStore((state) => state.hosts);
  const searchQuery = useHostStore((state) => state.searchQuery);
  const selectedHostId = useHostStore((state) => state.selectedHostId);
  const sessionsMap = useSessionStore((state) => state.sessions);
  const activeView = useSessionStore((state) => state.activeView);
  const monitorSnapshots = useMonitorStore((state) => state.snapshots);
  const selectedInterfaceName = useMonitorStore((state) => activeView === null ? null : state.selectedInterfaces.get(activeView) ?? null);
  const sftpStates = useSftpStore((state) => state.sessionStates);
  const sidebarWidth = useLayoutStore((state) => state.sidebarWidth);
  const collapsedGroups = useLayoutStore((state) => state.collapsedGroups);
  const monitorCollapsed = useLayoutStore((state) => state.monitorCollapsed);
  const theme = useThemeStore((state) => state.theme);
  const sessions = useMemo(() => [...sessionsMap.values()], [sessionsMap]);
  const snapshot = activeView === null ? null : monitorSnapshots.get(activeView) ?? null;
  const sftpState = activeView === null ? null : sftpStates.get(activeView) ?? null;
  const [editorOpen, setEditorOpen] = useState(false);
  const [editingHost, setEditingHost] = useState<HostConfig | null>(null);
  const [resizing, setResizing] = useState(false);
  const resizingRef = useRef(false);

  useEffect(() => {
    let disposed = false;
    const cleanups: Array<() => void> = [];
    /** 保存异步监听清理器；组件已卸载时立即执行。 */
    function keep(cleanup: () => void) {
      if (disposed) cleanup(); else cleanups.push(cleanup);
    }
    /** 拖动时按鼠标位置更新侧栏宽度。 */
    function move(event: PointerEvent) {
      if (resizingRef.current) useLayoutStore.getState().setSidebarWidth(event.clientX);
    }
    /** 停止侧栏拖动并清理全局样式。 */
    function stop() {
      resizingRef.current = false;
      setResizing(false);
      document.body.classList.remove('sidebar-resizing');
    }
    /** 窗口变化时重新限制侧栏宽度。 */
    function resize() {
      useLayoutStore.getState().syncSidebarWidthForViewport(window.innerWidth);
    }
    window.addEventListener('pointermove', move);
    window.addEventListener('pointerup', stop);
    window.addEventListener('resize', resize);
    resize();
    useSessionStore.getState().initListeners().then(keep);
    useMonitorStore.getState().initListeners().then(keep);
    useSftpStore.getState().initListeners().then(keep);
    useHostStore.getState().loadHosts();
    return () => {
      disposed = true;
      window.removeEventListener('pointermove', move);
      window.removeEventListener('pointerup', stop);
      window.removeEventListener('resize', resize);
      stop();
      cleanups.forEach((cleanup) => cleanup());
    };
  }, []);

  /** 打开指定主机的 SSH 会话。 */
  async function openSession(hostId: string) {
    await useSessionStore.getState().openSession(hostId);
  }

  /** 打开新建主机表单。 */
  function createHost() {
    setEditingHost(null);
    setEditorOpen(true);
  }

  /** 打开指定主机的编辑表单。 */
  function editHost(hostId: string) {
    const host = hosts.find((item) => item.id === hostId);
    if (!host) return;
    setEditingHost(host);
    setEditorOpen(true);
  }

  /** 保存主机并关闭编辑表单。 */
  async function saveHost(request: SaveHostRequest) {
    await useHostStore.getState().saveHost(request);
    setEditorOpen(false);
    setEditingHost(null);
  }

  /** 删除主机，并清理当前激活主机标识。 */
  async function removeHost(hostId: string) {
    await useHostStore.getState().deleteHost(hostId);
  }

  /** 重命名分组：更新主机归属并迁移折叠状态。 */
  async function renameGroup(oldName: string, newName: string) {
    await useHostStore.getState().renameGroup(oldName, newName);
    useLayoutStore.getState().renameCollapsedGroup(oldName, newName);
  }

  /** 删除分组：主机归入未分组并清除折叠状态。 */
  async function deleteGroup(name: string) {
    await useHostStore.getState().deleteGroup(name);
    useLayoutStore.getState().removeCollapsedGroup(name);
  }

  /** 启动侧栏宽度拖动。 */
  function startSidebarResize(event: React.PointerEvent) {
    resizingRef.current = true;
    setResizing(true);
    document.body.classList.add('sidebar-resizing');
    useLayoutStore.getState().setSidebarWidth(event.clientX);
  }

  /** 选择本地保存路径并逐个下载远程文件。 */
  async function download(sessionId: string, remotePaths: string[]) {
    for (const remotePath of remotePaths) {
      const localPath = await saveFileDialog({ defaultPath: remotePath.split('/').pop() ?? 'download' });
      if (localPath) await useSftpStore.getState().download(sessionId, remotePath, localPath);
    }
  }

  /** 选择单个本地文件并上传到当前远程目录。 */
  async function upload(sessionId: string, remotePath: string) {
    const localPath = await openFileDialog({ multiple: false, directory: false });
    if (typeof localPath === 'string') await useSftpStore.getState().upload(sessionId, localPath, remotePath);
  }

  /** 根据原任务方向重新发起传输。 */
  async function retry(task: TransferTask) {
    if (task.transferType === 'Download') {
      await useSftpStore.getState().download(task.sessionId, task.remotePath, task.localPath);
    } else {
      await useSftpStore.getState().upload(task.sessionId, task.localPath, task.remotePath);
    }
  }

  return <div className={`page-shell ${resizing ? 'page-shell--resizing' : ''}`}>
    <aside className="sidebar" style={{ width: sidebarWidth }}>
      <div className="sidebar-header">
        <Typography.Text type="secondary" className="brand">Titan SSH</Typography.Text>
      </div>
      <HostListSidebar hosts={filterHosts(hosts, searchQuery)} searchQuery={searchQuery} selectedHostId={selectedHostId}
        collapsedGroups={collapsedGroups}
        onToggleGroup={(name) => useLayoutStore.getState().toggleGroupCollapsed(name)}
        onRenameGroup={renameGroup} onDeleteGroup={deleteGroup}
        onEditHost={editHost} onDeleteHost={removeHost}
        onSearchChange={(query) => useHostStore.getState().setSearchQuery(query)}
        onSelect={(hostId) => useHostStore.getState().selectHost(hostId)}
        onOpen={openSession} onCreate={createHost} />
      <div className="sidebar-footer">
        {monitorCollapsed ? (
          <div className="sidebar-footer-row">
            <ServerStatusPanel snapshot={snapshot} selectedInterfaceName={selectedInterfaceName}
              onInterfaceChange={(name) => activeView && useMonitorStore.getState().selectNetworkInterface(activeView, name)}
              collapsed onToggle={() => useLayoutStore.getState().toggleMonitorCollapsed()} />
            <FooterActions theme={theme} />
          </div>
        ) : (
          <>
            <ServerStatusPanel snapshot={snapshot} selectedInterfaceName={selectedInterfaceName}
              onInterfaceChange={(name) => activeView && useMonitorStore.getState().selectNetworkInterface(activeView, name)}
              collapsed={false} onToggle={() => useLayoutStore.getState().toggleMonitorCollapsed()} />
            <div className="sidebar-footer-row sidebar-footer-row--right"><FooterActions theme={theme} /></div>
          </>
        )}
      </div>
    </aside>
    <div className="sidebar-resizer" role="separator" aria-orientation="vertical" aria-valuenow={sidebarWidth}
      aria-valuemin={220} aria-valuemax={clampSidebarWidth(MAX_SIDEBAR_WIDTH, window.innerWidth)} onPointerDown={startSidebarResize} />
    <section className="main-panel">
      {sessions.length > 0 && <div className="tabs-area"><TerminalTabs sessions={sessions} activeView={activeView}
        onActivate={(view) => useSessionStore.getState().setActiveView(view)}
        onClose={(sessionId) => useSessionStore.getState().closeSession(sessionId)} /></div>}
      <div className="content-area">
        <TerminalPane sessions={sessions} activeView={activeView}
          onInput={({ sessionId, data }) => useSessionStore.getState().writeTerminal(sessionId, data)}
          onResize={({ sessionId, cols, rows }) => useSessionStore.getState().resizeTerminal(sessionId, cols, rows)}
          onCreateHost={createHost} />
        {activeView !== null && <SftpPanel sessionId={activeView} state={sftpState}
          onNavigate={(sessionId, path) => useSftpStore.getState().listDir(sessionId, path)}
          onSelect={(sessionId, path) => useSftpStore.getState().toggleSelect(sessionId, path)}
          onDownload={download} onUpload={upload}
          onCancel={(taskId) => useSftpStore.getState().cancelTask(taskId)} onRetry={retry} />}
      </div>
    </section>
    <HostEditorDialog open={editorOpen} editingHost={editingHost} groups={useMemo(
      () => [...new Set(hosts.map((host) => host.group).filter(Boolean))], [hosts])}
      onClose={() => setEditorOpen(false)} onSave={saveHost} />
  </div>;
}

/** 侧栏底部全局入口：主题切换与设置占位。 */
function FooterActions({ theme }: { theme: string }) {
  return (
    <div className="sidebar-footer-actions">
      <button type="button" className="sidebar-footer-btn" data-testid="theme-toggle" aria-label="切换主题"
        onClick={() => useThemeStore.getState().toggleTheme()}>{theme === 'dark' ? <Moon size={14} /> : <Sun size={14} />}</button>
      <button type="button" className="sidebar-footer-btn" aria-label="设置" title="设置（即将推出）" disabled>
        <Settings size={14} />
      </button>
    </div>
  );
}
