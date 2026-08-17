import type { HostIdentityChallenge, SessionConnection, SessionInfo } from '@/types/session';
import { ConnectionPhase, SessionStatus } from '@/types/session';
import { connectionLabel, overlayStatus, progressLabel } from '@/stores/session';
import type { AppErrorInfo } from '@/i18n';
import EmptyState from '@/components/shell/EmptyState';
import HostIdentityCard from './HostIdentityCard';
import TerminalOverlay from './TerminalOverlay';
import XtermView from './XtermView';

interface Props {
  sessions: SessionInfo[];
  activeView: string | null;
  /** 按 sessionId 存储的连接生命周期投影（阶段 + 结构化错误）。 */
  connections: Map<string, SessionConnection>;
  /** 按 sessionId 存储的主机身份确认投影；存在时终端区域显示内联确认卡。 */
  challenges: Map<string, HostIdentityChallenge>;
  /** 按 sessionId 存储的"接受并保存"结构化失败；显示在所属标签的确认卡内。 */
  saveErrors: Map<string, AppErrorInfo>;
  onInput: (event: { sessionId: string; data: Uint8Array }) => void;
  onResize: (event: { sessionId: string; cols: number; rows: number }) => void;
  onCreateHost: () => void;
  onCloseTab: (sessionId: string) => void;
  onSaveIdentity: (sessionId: string) => void;
  onAcceptIdentity: (sessionId: string) => void;
  onRejectIdentity: (sessionId: string) => void;
}

/** 在空态页与各真实终端视图之间切换；会话实例常驻，仅切换显隐。
 *  每个标签独立呈现连接生命周期：主机身份确认卡 > 连接/错误覆盖层。 */
export default function TerminalPane({ sessions, activeView, connections, challenges, saveErrors, onInput, onResize, onCreateHost, onCloseTab, onSaveIdentity, onAcceptIdentity, onRejectIdentity }: Props) {
  return (
    <section className="terminal-pane">
      <div className="viewport">
        {activeView === null && <EmptyState onCreateHost={onCreateHost} />}
        {sessions.map((session) => {
          const challenge = challenges.get(session.sessionId);
          const connection = connections.get(session.sessionId);
          // 等待确认期间在确认卡内展示主机身份验证阶段（不渲染独立覆盖层）
          const phaseLabel = connection?.phase === ConnectionPhase.VerifyingHostKey
            ? progressLabel(connection.phase)
            : null;
          return (
            <div key={session.sessionId} className="terminal-session" hidden={activeView !== session.sessionId}>
              <XtermView sessionId={session.sessionId}
                active={activeView === session.sessionId}
                interactive={session.status === SessionStatus.Connected}
                onInput={onInput} onResize={onResize} />
              {challenge ? (
                <HostIdentityCard challenge={challenge} phaseLabel={phaseLabel}
                  saveError={saveErrors.get(session.sessionId)}
                  onAcceptAndSave={() => onSaveIdentity(session.sessionId)}
                  onAccept={() => onAcceptIdentity(session.sessionId)}
                  onReject={() => onRejectIdentity(session.sessionId)} />
              ) : overlayStatus(session.status) && (
                <TerminalOverlay status={session.status}
                  message={connectionLabel(session, connection)}
                  onClose={() => onCloseTab(session.sessionId)} />
              )}
            </div>
          );
        })}
      </div>
    </section>
  );
}
