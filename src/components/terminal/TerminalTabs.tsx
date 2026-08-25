import { X } from 'lucide-react';
import type { MouseEvent } from 'react';
import type { SessionInfo } from '@/types/session';
import type { TerminalTab } from '@/types/tab';
import { translate } from '@/i18n';
import { useLocaleStore } from '@/stores/locale';

interface Props {
  /** 标签列表（渲染源）：标签引用会话但不拥有连接；按插入顺序渲染 */
  tabs: TerminalTab[];
  /** 按 sessionId 索引的会话投影；标签据此解析标题文案与状态圆点 */
  sessions: Map<string, SessionInfo>;
  activeTabId: string | null;
  onActivate: (tabId: string) => void;
  onClose: (tabId: string) => void;
}

/** 根据会话状态返回状态圆点样式。 */
function statusDot(status: string) {
  if (status === 'Connected') return 'dot-connected';
  if (status === 'Connecting') return 'dot-connecting';
  if (['AuthFailed', 'Error', 'Timeout', 'Disconnected'].includes(status)) return 'dot-error';
  return 'dot-offline';
}

/** 渲染标签栏（无标签时整栏隐藏）：终端与纯视图标签共用会话投影解析展示信息。 */
export default function TerminalTabs({ tabs, sessions, activeTabId, onActivate, onClose }: Props) {
  const locale = useLocaleStore((state) => state.locale);
  /** 关闭标签且不触发激活。 */
  function closeTab(event: MouseEvent, tab: TerminalTab) {
    event.stopPropagation();
    onClose(tab.tabId);
  }

  return (
    <div className="tab-bar" role="tablist">
      {tabs.map((tab) => {
        // 标签与其会话投影同生命周期（removeSessionProjection 一并移除），缺投影时不渲染
        const session = sessions.get(tab.sessionId);
        if (!session) return null;
        const sessionName = `${session.username}@${session.host}`;
        const tabName = tab.type === 'process' ? `${translate(locale, 'process.tab')} · ${sessionName}` : sessionName;
        return (
          <div key={tab.tabId} className={`tab ${activeTabId === tab.tabId ? 'active' : ''}`}
            role="tab" aria-selected={activeTabId === tab.tabId} onClick={() => onActivate(tab.tabId)}>
            <span className={`status-dot ${statusDot(session.status)}`} />
            <span className="tab-label">{tabName}</span>
            <button type="button" className="close-btn" aria-label={translate(locale, 'tab.close', { name: tabName })}
              onClick={(event) => closeTab(event, tab)}><X size={11} /></button>
          </div>
        );
      })}
    </div>
  );
}
