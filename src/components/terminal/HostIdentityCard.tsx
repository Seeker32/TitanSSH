import type { HostIdentityChallenge } from '@/types/session';
import { formatAppError, translate, type AppErrorInfo } from '@/i18n';
import { useLocaleStore } from '@/stores/locale';

interface Props {
  challenge: HostIdentityChallenge;
  /** 验证阶段的进度文案（如"正在验证主机身份..."）；非验证阶段为 null 不展示。 */
  phaseLabel?: string | null;
  /** "接受并保存"失败的结构化错误；challenge 保持未决，错误显示在所属标签内。 */
  saveError?: AppErrorInfo | null;
  onAcceptAndSave: () => void;
  onAccept: () => void;
  onReject: () => void;
}

/** 在所属终端区域内联呈现首次主机身份确认卡：endpoint、算法与 SHA-256 指纹，
 *  等待期间展示主机身份验证阶段，提供"接受并保存""仅本次接受"与"拒绝"操作；
 *  保存失败保持未决并显示结构化错误，绝不静默降级为临时信任；不使用全局 Modal。 */
export default function HostIdentityCard({ challenge, phaseLabel, saveError, onAcceptAndSave, onAccept, onReject }: Props) {
  const locale = useLocaleStore((state) => state.locale);
  return (
    <div className="terminal-overlay terminal-overlay--identity" data-testid="host-identity-card"
      role="alertdialog" aria-label={translate(locale, 'hostIdentity.title')}>
      <div className="host-identity-card">
        {phaseLabel && <p className="host-identity-card__phase">{phaseLabel}</p>}
        <p className="host-identity-card__title">{translate(locale, 'hostIdentity.title')}</p>
        <p className="host-identity-card__hint">{translate(locale, 'hostIdentity.hint')}</p>
        <dl className="host-identity-card__meta">
          <div className="host-identity-card__row">
            <dt>{translate(locale, 'hostIdentity.endpoint')}</dt>
            <dd className="host-identity-card__mono">{challenge.host}:{challenge.port}</dd>
          </div>
          <div className="host-identity-card__row">
            <dt>{translate(locale, 'hostIdentity.algorithm')}</dt>
            <dd className="host-identity-card__mono">{challenge.keyAlgorithm}</dd>
          </div>
          <div className="host-identity-card__row">
            <dt>{translate(locale, 'hostIdentity.fingerprint')}</dt>
            <dd className="host-identity-card__mono">{challenge.fingerprint}</dd>
          </div>
        </dl>
        {saveError && (
          <div className="host-identity-card__save-error" data-testid="host-identity-save-error" role="alert">
            <p>{translate(locale, 'hostIdentity.saveFailed')}</p>
            <p className="host-identity-card__mono">{formatAppError(locale, saveError)}</p>
          </div>
        )}
        <div className="host-identity-card__actions">
          <button type="button" className="host-identity-card__save" data-testid="host-identity-save"
            onClick={onAcceptAndSave}>{translate(locale, 'hostIdentity.acceptAndSave')}</button>
          <button type="button" className="host-identity-card__accept" data-testid="host-identity-accept"
            onClick={onAccept}>{translate(locale, 'hostIdentity.accept')}</button>
          <button type="button" className="host-identity-card__reject" data-testid="host-identity-reject"
            onClick={onReject}>{translate(locale, 'hostIdentity.reject')}</button>
        </div>
      </div>
    </div>
  );
}
