import type { SessionInfo } from '@/types/session';
import EmptyState from '@/components/shell/EmptyState';
import XtermView from './XtermView';

interface Props {
  sessions: SessionInfo[];
  activeView: string | null;
  onInput: (event: { sessionId: string; data: string }) => void;
  onResize: (event: { sessionId: string; cols: number; rows: number }) => void;
  onCreateHost: () => void;
}

/** 在空态页与各真实终端视图之间切换；会话实例常驻，仅切换显隐。 */
export default function TerminalPane({ sessions, activeView, onInput, onResize, onCreateHost }: Props) {
  return (
    <section className="terminal-pane">
      <div className="viewport">
        {activeView === null && <EmptyState onCreateHost={onCreateHost} />}
        {sessions.map((session) => (
          <XtermView key={session.sessionId} sessionId={session.sessionId}
            active={activeView === session.sessionId} onInput={onInput} onResize={onResize} />
        ))}
      </div>
    </section>
  );
}
