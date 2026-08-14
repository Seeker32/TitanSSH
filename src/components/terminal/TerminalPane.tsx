import type { SessionConnection, SessionInfo } from '@/types/session';
import { SessionStatus } from '@/types/session';
import { connectionLabel, overlayStatus } from '@/stores/session';
import EmptyState from '@/components/shell/EmptyState';
import TerminalOverlay from './TerminalOverlay';
import XtermView from './XtermView';

interface Props {
  sessions: SessionInfo[];
  activeView: string | null;
  /** 按 sessionId 存储的连接生命周期投影（阶段 + 结构化错误）。 */
  connections: Map<string, SessionConnection>;
  onInput: (event: { sessionId: string; data: string }) => void;
  onResize: (event: { sessionId: string; cols: number; rows: number }) => void;
  onCreateHost: () => void;
  onCloseTab: (sessionId: string) => void;
}

/** 在空态页与各真实终端视图之间切换；会话实例常驻，仅切换显隐，且每个标签独立呈现连接生命周期。 */
export default function TerminalPane({ sessions, activeView, connections, onInput, onResize, onCreateHost, onCloseTab }: Props) {
  return (
    <section className="terminal-pane">
      <div className="viewport">
        {activeView === null && <EmptyState onCreateHost={onCreateHost} />}
        {sessions.map((session) => (
          <div key={session.sessionId} className="terminal-session" hidden={activeView !== session.sessionId}>
            <XtermView sessionId={session.sessionId}
              active={activeView === session.sessionId}
              interactive={session.status === SessionStatus.Connected}
              onInput={onInput} onResize={onResize} />
            {overlayStatus(session.status) && (
              <TerminalOverlay status={session.status}
                message={connectionLabel(session, connections.get(session.sessionId))}
                onClose={() => onCloseTab(session.sessionId)} />
            )}
          </div>
        ))}
      </div>
    </section>
  );
}
