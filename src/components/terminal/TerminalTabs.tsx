import { Button } from 'antd';
import type { MouseEvent } from 'react';
import type { SessionInfo } from '@/types/session';

interface Props {
  sessions: SessionInfo[];
  activeView: 'home' | string;
  onActivate: (viewId: 'home' | string) => void;
  onClose: (sessionId: string) => void;
}

/** 根据会话状态返回状态圆点样式。 */
function statusDot(status: string) {
  if (status === 'Connected') return 'dot-connected';
  if (status === 'Connecting') return 'dot-connecting';
  if (['AuthFailed', 'Error', 'Timeout', 'Disconnected'].includes(status)) return 'dot-error';
  return 'dot-offline';
}

/** 渲染固定首页和真实 SSH 会话标签。 */
export default function TerminalTabs({ sessions, activeView, onActivate, onClose }: Props) {
  /** 关闭标签且不触发激活。 */
  function closeTab(event: MouseEvent, sessionId: string) {
    event.stopPropagation();
    onClose(sessionId);
  }

  return (
    <div className="tab-bar" role="tablist">
      <div className={`tab ${activeView === 'home' ? 'active' : ''}`} role="tab"
        aria-selected={activeView === 'home'} onClick={() => onActivate('home')}>
        <span className="tab-label">首页</span>
      </div>
      {sessions.map((session) => (
        <div key={session.sessionId} className={`tab ${activeView === session.sessionId ? 'active' : ''}`}
          role="tab" aria-selected={activeView === session.sessionId} onClick={() => onActivate(session.sessionId)}>
          <span className={`status-dot ${statusDot(session.status)}`} />
          <span className="tab-label">{session.username}@{session.host}</span>
          <Button type="text" size="small" className="close-btn" aria-label={`关闭 ${session.username}@${session.host}`}
            onClick={(event) => closeTab(event, session.sessionId)}>×</Button>
        </div>
      ))}
    </div>
  );
}
