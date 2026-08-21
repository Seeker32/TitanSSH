import { Moon, Settings, Sun } from 'lucide-react';
import { useEffect, useMemo, useRef, useState } from 'react';
import { Modal, Select, Typography } from 'antd';
import { open as openFileDialog, save as saveFileDialog } from '@tauri-apps/plugin-dialog';
import { LOG_LEVELS, useLogLevelStore, type LogLevel } from '@/stores/log-level';
import HostEditorDialog from '@/components/host/HostEditorDialog';
import HostListSidebar from '@/components/host/HostListSidebar';
import SftpPanel from '@/components/sftp/SftpPanel';
import RecentTransfers from '@/components/sftp/RecentTransfers';
import ServerStatusPanel from '@/components/status/ServerStatusPanel';
import TerminalPane from '@/components/terminal/TerminalPane';
import TerminalTabs from '@/components/terminal/TerminalTabs';
import TrustedHostsSection from '@/components/settings/TrustedHostsSection';
import LogViewer from '@/components/settings/LogViewer';
import { filterHosts, useHostStore } from '@/stores/host';
import { useLayoutStore } from '@/stores/layout';
import { useMonitorStore } from '@/stores/monitor';
import { useSessionStore } from '@/stores/session';
import { useSftpStore } from '@/stores/sftp';
import { useThemeStore } from '@/stores/theme';
import { useTerminalThemeStore } from '@/stores/terminal-theme';
import { useTrustedHostsStore } from '@/stores/trusted-hosts';
import { translate, toAppError, type AppErrorInfo } from '@/i18n';
import { useLocaleStore } from '@/stores/locale';
import { TERMINAL_THEME_NAMES, terminalThemes } from '@/components/terminal/terminalThemes';
import type { HostConfig, SaveHostRequest } from '@/types/host';
import { uploadTargetDir, type TransferTask } from '@/types/sftp';

/** 协调主机、会话、终端、监控与 SFTP 视图。 */
export default function HomePage() {
  const hosts = useHostStore((state) => state.hosts);
  const searchQuery = useHostStore((state) => state.searchQuery);
  const selectedHostId = useHostStore((state) => state.selectedHostId);
  const sessionsMap = useSessionStore((state) => state.sessions);
  const tabsMap = useSessionStore((state) => state.tabs);
  const activeTabId = useSessionStore((state) => state.activeTabId);
  const connections = useSessionStore((state) => state.connections);
  const hostKeyChallenges = useSessionStore((state) => state.hostKeyChallenges);
  const hostKeySaveErrors = useSessionStore((state) => state.hostKeySaveErrors);
  const monitorSnapshots = useMonitorStore((state) => state.snapshots);
  const sftpStates = useSftpStore((state) => state.sessionStates);
  const recentTransfers = useSftpStore((state) => state.recentTransfers);
  const sidebarWidth = useLayoutStore((state) => state.sidebarWidth);
  const collapsedGroups = useLayoutStore((state) => state.collapsedGroups);
  const monitorCollapsed = useLayoutStore((state) => state.monitorCollapsed);
  const theme = useThemeStore((state) => state.theme);
  // 标签视图模型（ADR-0002）：标签栏与内容区从标签列表渲染；激活会话由激活标签派生
  const tabs = useMemo(() => [...tabsMap.values()], [tabsMap]);
  const activeTab = activeTabId === null ? null : tabsMap.get(activeTabId) ?? null;
  const activeView = activeTab === null ? null : activeTab.sessionId;
  const selectedInterfaceName = useMonitorStore((state) => activeView === null ? null : state.selectedInterfaces.get(activeView) ?? null);
  const trendSamples = useMonitorStore((state) => activeView === null ? undefined : state.networkTrends.get(activeView));
  const snapshot = activeView === null ? null : monitorSnapshots.get(activeView) ?? null;
  const sftpState = activeView === null ? null : sftpStates.get(activeView) ?? null;
  const [editorOpen, setEditorOpen] = useState(false);
  const [editingHost, setEditingHost] = useState<HostConfig | null>(null);
  /** 最近一次主机保存失败的结构化错误；在编辑弹窗内展示，保存成功后清空 */
  const [editorError, setEditorError] = useState<AppErrorInfo | null>(null);
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
    setEditorError(null);
    setEditorOpen(true);
  }

  /** 打开指定主机的编辑表单。 */
  function editHost(hostId: string) {
    const host = hosts.find((item) => item.id === hostId);
    if (!host) return;
    setEditingHost(host);
    setEditorError(null);
    setEditorOpen(true);
  }

  /** 保存主机并关闭编辑表单；失败时在弹窗内展示错误，弹窗保持打开。 */
  async function saveHost(request: SaveHostRequest) {
    try {
      await useHostStore.getState().saveHost(request);
      setEditorOpen(false);
      setEditingHost(null);
    } catch (error) {
      setEditorError(toAppError(error));
    }
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

  /** 启动侧栏宽度拖动：阻止默认行为防止拖动过程中文本被选中。 */
  function startSidebarResize(event: React.PointerEvent<HTMLDivElement>) {
    event.preventDefault();
    resizingRef.current = true;
    document.body.classList.add('sidebar-resizing');
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

  /** 根据原任务方向重新发起传输；失败在原任务行与文件浏览器错误区可见。
   *  上传的 remotePath 为完整目标路径，重试必须回到其目标目录。 */
  async function retry(task: TransferTask) {
    if (task.transferType === 'Download') {
      await useSftpStore.getState().download(task.sessionId, task.remotePath, task.localPath, task.taskId);
    } else {
      await useSftpStore.getState().upload(task.sessionId, task.localPath, uploadTargetDir(task), task.taskId);
    }
  }

  /** 对单个冲突文件确认覆盖：以 Overwrite 策略按方向重新发起传输，不扩展到批次或会话。 */
  async function overwrite(task: TransferTask) {
    if (task.transferType === 'Download') {
      await useSftpStore.getState().download(task.sessionId, task.remotePath, task.localPath, task.taskId, 'Overwrite');
    } else {
      await useSftpStore.getState().upload(task.sessionId, task.localPath, uploadTargetDir(task), task.taskId, 'Overwrite');
    }
  }

  return <div className="page-shell">
    <aside className="sidebar" style={{ width: sidebarWidth }}>
      <div data-testid="sidebar-resizer" className="sidebar-resizer" role="separator" aria-orientation="vertical" onPointerDown={startSidebarResize} />
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
      <RecentTransfers tasks={recentTransfers} onClear={() => useSftpStore.getState().clearRecentTransfers()} />
      <div className="sidebar-footer">
        {monitorCollapsed ? (
          <div className="sidebar-footer-row">
            <ServerStatusPanel snapshot={snapshot} selectedInterfaceName={selectedInterfaceName}
              onInterfaceChange={(name) => activeView && useMonitorStore.getState().selectNetworkInterface(activeView, name)}
              trendSamples={trendSamples}
              collapsed onToggle={() => useLayoutStore.getState().toggleMonitorCollapsed()} />
            <FooterActions theme={theme} />
          </div>
        ) : (
          <>
            <ServerStatusPanel snapshot={snapshot} selectedInterfaceName={selectedInterfaceName}
              onInterfaceChange={(name) => activeView && useMonitorStore.getState().selectNetworkInterface(activeView, name)}
              trendSamples={trendSamples}
              collapsed={false} onToggle={() => useLayoutStore.getState().toggleMonitorCollapsed()} />
            <div className="sidebar-footer-row sidebar-footer-row--right"><FooterActions theme={theme} /></div>
          </>
        )}
      </div>
    </aside>
    <section className="main-panel">
      {tabs.length > 0 && <div className="tabs-area"><TerminalTabs tabs={tabs} sessions={sessionsMap} activeTabId={activeTabId}
        onActivate={(tabId) => useSessionStore.getState().setActiveTab(tabId)}
        onClose={(tabId) => useSessionStore.getState().closeTab(tabId)} /></div>}
      <div className="content-area">
        <TerminalPane tabs={tabs} sessions={sessionsMap} activeTabId={activeTabId} connections={connections}
          challenges={hostKeyChallenges} saveErrors={hostKeySaveErrors}
          onInput={({ sessionId, data }) => useSessionStore.getState().writeTerminal(sessionId, data)}
          onResize={({ sessionId, cols, rows }) => useSessionStore.getState().resizeTerminal(sessionId, cols, rows)}
          onCreateHost={createHost}
          onCloseTab={(tabId) => useSessionStore.getState().closeTab(tabId)}
          onSaveIdentity={(sessionId) => useSessionStore.getState().acceptAndSaveHostIdentity(sessionId)}
          onAcceptIdentity={(sessionId) => useSessionStore.getState().acceptHostIdentity(sessionId)}
          onRejectIdentity={(sessionId) => useSessionStore.getState().rejectHostIdentity(sessionId)} />
        {activeView !== null && <SftpPanel sessionId={activeView} state={sftpState}
          onNavigate={(sessionId, path) => useSftpStore.getState().listDir(sessionId, path)}
          onSelect={(sessionId, path) => useSftpStore.getState().toggleSelect(sessionId, path)}
          onDownload={download} onUpload={upload}
          onCancel={(taskId) => useSftpStore.getState().cancelTask(taskId, activeView)} onRetry={retry}
          onOverwrite={overwrite}
          onClearTerminal={() => useSftpStore.getState().clearTerminalTasks(activeView)} />}
      </div>
    </section>
    <HostEditorDialog open={editorOpen} editingHost={editingHost} saveError={editorError} groups={useMemo(
      () => [...new Set(hosts.map((host) => host.group).filter(Boolean))], [hosts])}
      onClose={() => setEditorOpen(false)} onSave={saveHost} />
  </div>;
}

/** 侧栏底部全局入口：应用主题切换与 SSH 终端主题设置。 */
function FooterActions({ theme }: { theme: string }) {
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [settingsSection, setSettingsSection] = useState<'general' | 'terminal' | 'trustedHosts' | 'logging'>('general');
  const terminalTheme = useTerminalThemeStore((state) => state.terminalTheme);
  const logLevel = useLogLevelStore((state) => state.logLevel);
  const locale = useLocaleStore((state) => state.locale);

  useEffect(() => { void useLogLevelStore.getState().setLogLevel(logLevel); }, []);

  // 每次打开/切换到“可信主机”区域都重新加载：保存/替换/自动清理后清单反映后端当前记录
  useEffect(() => {
    if (settingsOpen && settingsSection === 'trustedHosts') {
      void useTrustedHostsStore.getState().load();
    }
  }, [settingsOpen, settingsSection]);

  /** 保存日志等级并立即同步后端日志过滤器。 */
  function setLogLevel(level: LogLevel) {
    void useLogLevelStore.getState().setLogLevel(level);
  }

  return (
    <div className="sidebar-footer-actions">
      <button type="button" className="sidebar-footer-btn" data-testid="theme-toggle" aria-label={translate(locale, 'theme.toggle')}
        onClick={() => useThemeStore.getState().toggleTheme()}>{theme === 'dark' ? <Moon size={14} /> : <Sun size={14} />}</button>
      <button type="button" className="sidebar-footer-btn" aria-label={translate(locale, 'settings.title')} title={translate(locale, 'settings.title')} onClick={() => setSettingsOpen(true)}>
        <Settings size={14} />
      </button>
      {/* destroyOnHidden：关闭后卸载子节点，日志查看器随之卸载并停止 2 秒轮询
          （antd v6 Modal 子节点被 memo 包装，open 变化不会触发子节点重渲染，条件渲染无效） */}
      <Modal open={settingsOpen} destroyOnHidden title={translate(locale, 'settings.title')} footer={null} width={680} onCancel={() => setSettingsOpen(false)}>
        <div className="settings-layout">
          <nav className="settings-nav" aria-label={translate(locale, 'settings.title')}>
            {(['general', 'terminal', 'trustedHosts', 'logging'] as const).map((section) => <button key={section} type="button" data-testid={`settings-section-${section}`}
              className={settingsSection === section ? 'settings-nav-btn settings-nav-btn--active' : 'settings-nav-btn'}
              aria-current={settingsSection === section ? 'page' : undefined} onClick={() => setSettingsSection(section)}>
              {translate(locale, `settings.${section}` as Parameters<typeof translate>[1])}
            </button>)}
          </nav>
          <section className="settings-content">
            {settingsSection === 'general' && <label className="settings-field">{translate(locale, 'settings.language')} <Select value={locale} onChange={(value) => useLocaleStore.getState().setLocale(value)}
              options={[{ value: 'zh-CN', label: translate(locale, 'locale.zh-CN') }, { value: 'en-US', label: translate(locale, 'locale.en-US') }]} /></label>}
            {settingsSection === 'terminal' && <>
              <Typography.Text type="secondary">{translate(locale, 'settings.terminalTheme')}</Typography.Text>
              <div className="terminal-theme-options">
                {TERMINAL_THEME_NAMES.map((name) => {
                  const palette = terminalThemes[name];
                  const selected = name === terminalTheme;
                  return <button key={name} type="button" aria-pressed={selected}
                    aria-label={`${translate(locale, 'settings.terminalTheme')}: ${translate(locale, `terminalTheme.${name}` as Parameters<typeof translate>[1])}`} className="terminal-theme-card"
                    onClick={() => useTerminalThemeStore.getState().setTerminalTheme(name)}>
                    <span className="terminal-theme-preview" style={{ background: palette.background, color: palette.foreground }}>
                      <span>$ ssh titan</span><span style={{ color: palette.green }}>connected</span>
                    </span>
                    <span>{translate(locale, `terminalTheme.${name}` as Parameters<typeof translate>[1])}</span>
                    {selected && <span className="terminal-theme-card__selected">{translate(locale, 'settings.selected')}</span>}
                  </button>;
                })}
              </div>
            </>}
            {settingsSection === 'trustedHosts' && <TrustedHostsSection onRetry={() => void useTrustedHostsStore.getState().load()} />}
            {settingsSection === 'logging' && <>
              <label className="settings-field">{translate(locale, 'settings.logLevel')}
                <select value={logLevel} onChange={(event) => setLogLevel(event.target.value as LogLevel)}>
                  {LOG_LEVELS.map((level) => <option key={level} value={level}>{translate(locale, `settings.logLevel.${level}` as Parameters<typeof translate>[1])}</option>)}
                </select>
              </label>
              <LogViewer />
            </>}
          </section>
        </div>
      </Modal>
    </div>
  );
}
