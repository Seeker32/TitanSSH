import type { HostConfig } from '@/types/host';
import type { SessionInfo } from '@/types/session';
import HomeQuickActions from '@/components/home/HomeQuickActions';
import XtermView from './XtermView';

interface Props {
  sessions: SessionInfo[];
  activeView: 'home' | string;
  hosts: HostConfig[];
  onInput: (event: { sessionId: string; data: string }) => void;
  onResize: (event: { sessionId: string; cols: number; rows: number }) => void;
  onOpenHost: (hostId: string) => void;
  onEditHost: (hostId: string) => void;
  onRemoveHost: (hostId: string) => void;
  onCreateHost: () => void;
}

/** 在首页快捷入口与各真实终端视图之间切换。 */
export default function TerminalPane(props: Props) {
  return (
    <section className="terminal-pane">
      <div className="viewport">
        <div className="home-view" hidden={props.activeView !== 'home'}>
          <HomeQuickActions hosts={props.hosts} onOpen={props.onOpenHost} onEdit={props.onEditHost}
            onRemove={props.onRemoveHost} onCreate={props.onCreateHost} />
        </div>
        {props.sessions.map((session) => (
          <XtermView key={session.sessionId} sessionId={session.sessionId}
            active={props.activeView === session.sessionId} onInput={props.onInput} onResize={props.onResize} />
        ))}
      </div>
    </section>
  );
}
