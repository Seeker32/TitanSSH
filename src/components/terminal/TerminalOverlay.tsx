import type { SessionStatus } from '@/types/session';
import { translate } from '@/i18n';
import { useLocaleStore } from '@/stores/locale';

interface Props {
  status: SessionStatus;
  message: string;
  onClose: () => void;
}

/** 在所属终端区域呈现连接生命周期覆盖层：Connecting 显示阶段加载动画，失败显示结构化错误且仅提供关闭标签操作。 */
export default function TerminalOverlay({ status, message, onClose }: Props) {
  const locale = useLocaleStore((state) => state.locale);
  if (status === 'Connecting') {
    return (
      <div className="terminal-overlay terminal-overlay--connecting" role="status" aria-live="polite">
        <span className="spinner" aria-hidden="true" />
        <span className="terminal-overlay__message">{message}</span>
      </div>
    );
  }
  return (
    <div className="terminal-overlay terminal-overlay--error" role="alert">
      <p className="terminal-overlay__message terminal-overlay__message--error">{message}</p>
      <button type="button" className="terminal-overlay__close" aria-label={translate(locale, 'session.closeTab')}
        onClick={onClose}>{translate(locale, 'session.closeTab')}</button>
    </div>
  );
}
