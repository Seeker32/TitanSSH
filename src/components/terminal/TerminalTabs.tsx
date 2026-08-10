import { X } from 'lucide-react';
import type { MouseEvent } from 'react';
import type { SessionInfo } from '@/types/session';
import { translate } from '@/i18n';
import { useLocaleStore } from '@/stores/locale';

interface Props {
  sessions: SessionInfo[];
  activeView: string | null;
  onActivate: (viewId: string) => void;
  onClose: (sessionId: string) => void;
}

/** 根据会话状态返回状态圆点样式。 */
function statusDot(status: string) {
  if (status === 'Connected') return 'dot-connected';
  if (status === 'Connecting') return 'dot-connecting';
  if (['AuthFailed', 'Error', 'Timeout', 'Disconnected'].includes(status)) return 'dot-error';
  return 'dot-offline';
}

/** 渲染真实 SSH 会话标签栏（无会话时整栏隐藏）。 */
export default function TerminalTabs({ sessions, activeView, onActivate, onClose }: Props) {
  const locale = useLocaleStore((state) => state.locale);
  /** 关闭标签且不触发激活。 */
  function closeTab(event: MouseEvent, sessionId: string) {
    event.stopPropagation();
    onClose(sessionId);
  }

  return (
    <div className="tab-bar" role="tablist">
      {sessions.map((session) => (
        <div key={session.sessionId} className={`tab ${activeView === session.sessionId ? 'active' : ''}`}
          role="tab" aria-selected={activeView === session.sessionId} onClick={() => onActivate(session.sessionId)}>
          <span className={`status-dot ${statusDot(session.status)}`} />
          <span className="tab-label">{session.username}@{session.host}</span>
          <button type="button" className="close-btn" aria-label={translate(locale, 'tab.close', { name: `${session.username}@${session.host}` })}
            onClick={(event) => closeTab(event, session.sessionId)}><X size={11} /></button>
        </div>
      ))}
    </div>
  );
}
