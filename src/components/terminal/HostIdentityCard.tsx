import { useState } from 'react';
import { Alert, Button, Card, Spin } from 'antd';
import type { HostIdentityChallenge } from '@/types/session';
import { formatAppError, translate, type AppErrorInfo, type TranslationKey } from '@/i18n';
import { useLocaleStore } from '@/stores/locale';

interface Props {
  challenge: HostIdentityChallenge;
  /** 验证阶段的进度文案（如"正在验证主机身份..."）；非验证阶段为 null 不展示。 */
  phaseLabel?: string | null;
  /** "接受并保存"/"替换记录"失败的结构化错误；challenge 保持未决，错误显示在所属标签内。 */
  saveError?: AppErrorInfo | null;
  onAcceptAndSave: () => void;
  onAccept: () => void;
  onReject: () => void;
}

/** 判断 challenge 是否为"已保存 key 与呈现不一致"；旧后端缺省视为 Unknown。 */
function isChanged(challenge: HostIdentityChallenge): boolean {
  return challenge.kind === 'Changed';
}

/** 在所属终端区域内联呈现主机身份确认卡：endpoint、算法与 SHA-256 指纹，
 *  等待期间展示主机身份验证阶段。
 *  Unknown 提供"接受并保存""仅本次接受"与"拒绝"；Changed 同时展示已保存旧记录与
 *  服务器呈现的算法/指纹，提供"仅本次接受""替换记录"与"拒绝"，替换必须经过
 *  第二次内联确认（不要求手动输入指纹）。保存/替换失败保持未决并显示结构化错误，
 *  绝不静默降级为临时信任；不使用全局 Modal。 */
export default function HostIdentityCard({ challenge, phaseLabel, saveError, onAcceptAndSave, onAccept, onReject }: Props) {
  const locale = useLocaleStore((state) => state.locale);
  const t = (key: TranslationKey) => translate(locale, key);
  const changed = isChanged(challenge);
  // 替换记录的第二次内联确认状态。challenge 更换（服务端再次换 key 等）时在渲染期
  // 同步重置（React 渲染期状态调整模式，避免多一次提交/闪帧）；保存失败时退回主操作区，
  // 错误就地展示，用户可直接重试替换或改选仅本次接受/拒绝。
  const [confirmState, setConfirmState] = useState(() => ({
    challengeId: challenge.challengeId,
    confirming: false,
  }));
  if (confirmState.challengeId !== challenge.challengeId) {
    setConfirmState({ challengeId: challenge.challengeId, confirming: false });
  }
  const confirmingReplace = confirmState.confirming && !saveError;

  return (
    <div className="terminal-overlay terminal-overlay--identity" data-testid="host-identity-card"
      role="alertdialog" aria-label={t(changed ? 'hostIdentity.changedTitle' : 'hostIdentity.title')}>
        <Card className={`host-identity-card${changed ? ' host-identity-card--changed' : ''}`} variant="borderless">
        {phaseLabel && <div className="host-identity-card__phase"><Spin size="small" />{phaseLabel}</div>}
        <p className="host-identity-card__title">{t(changed ? 'hostIdentity.changedTitle' : 'hostIdentity.title')}</p>
        <p className="host-identity-card__hint">{t(changed ? 'hostIdentity.changedHint' : 'hostIdentity.hint')}</p>
        <dl className="host-identity-card__meta">
          <div className="host-identity-card__row">
            <dt>{t('hostIdentity.endpoint')}</dt>
            <dd className="host-identity-card__mono">{challenge.host}:{challenge.port}</dd>
          </div>
          {changed ? (
            <>
              <div className="host-identity-card__group host-identity-card__group--stored" data-testid="host-identity-stored">
                <div className="host-identity-card__row">
                  <dt>{t('hostIdentity.storedAlgorithm')}</dt>
                  <dd className="host-identity-card__mono">{challenge.storedAlgorithm}</dd>
                </div>
                <div className="host-identity-card__row">
                  <dt>{t('hostIdentity.storedFingerprint')}</dt>
                  <dd className="host-identity-card__mono">{challenge.storedFingerprint}</dd>
                </div>
              </div>
              <div className="host-identity-card__group host-identity-card__group--presented" data-testid="host-identity-presented">
                <div className="host-identity-card__row">
                  <dt>{t('hostIdentity.presentedAlgorithm')}</dt>
                  <dd className="host-identity-card__mono">{challenge.keyAlgorithm}</dd>
                </div>
                <div className="host-identity-card__row">
                  <dt>{t('hostIdentity.presentedFingerprint')}</dt>
                  <dd className="host-identity-card__mono">{challenge.fingerprint}</dd>
                </div>
              </div>
            </>
          ) : (
            <>
              <div className="host-identity-card__row">
                <dt>{t('hostIdentity.algorithm')}</dt>
                <dd className="host-identity-card__mono">{challenge.keyAlgorithm}</dd>
              </div>
              <div className="host-identity-card__row">
                <dt>{t('hostIdentity.fingerprint')}</dt>
                <dd className="host-identity-card__mono">{challenge.fingerprint}</dd>
              </div>
            </>
          )}
        </dl>
        {saveError && (
          <div data-testid="host-identity-save-error">
            <Alert className="host-identity-card__save-error" type="error" showIcon
              title={t(changed ? 'hostIdentity.replaceFailed' : 'hostIdentity.saveFailed')}
              description={<span className="host-identity-card__mono">{formatAppError(locale, saveError)}</span>} />
          </div>
        )}
        <div className="host-identity-card__actions">
          {changed && confirmingReplace ? (
            <>
              <div className="host-identity-card__confirm" data-testid="host-identity-replace-confirm">
                <p className="host-identity-card__confirm-title">{t('hostIdentity.replaceConfirmTitle')}</p>
                <p className="host-identity-card__confirm-hint">{t('hostIdentity.replaceConfirmHint')}</p>
              </div>
              <Button type="primary" danger className="host-identity-card__replace-confirm" data-testid="host-identity-replace-confirm-btn"
                aria-label={t('hostIdentity.confirmReplace')} onClick={onAcceptAndSave}>{t('hostIdentity.confirmReplace')}</Button>
              <Button className="host-identity-card__cancel" data-testid="host-identity-replace-cancel" aria-label={t('hostIdentity.cancel')}
                onClick={() => setConfirmState((state) => ({ ...state, confirming: false }))}>{t('hostIdentity.cancel')}</Button>
            </>
          ) : changed ? (
            <>
              <Button type="primary" className="host-identity-card__accept" data-testid="host-identity-accept"
                aria-label={t('hostIdentity.accept')} onClick={onAccept}>{t('hostIdentity.accept')}</Button>
              <Button className="host-identity-card__save" data-testid="host-identity-replace"
                aria-label={t('hostIdentity.replaceRecord')} onClick={() => setConfirmState((state) => ({ ...state, confirming: true }))}>{t('hostIdentity.replaceRecord')}</Button>
              <Button className="host-identity-card__reject" data-testid="host-identity-reject" aria-label={t('hostIdentity.reject')}
                onClick={onReject}>{t('hostIdentity.reject')}</Button>
            </>
          ) : (
            <>
              <Button type="primary" className="host-identity-card__save" data-testid="host-identity-save"
                aria-label={t('hostIdentity.acceptAndSave')} onClick={onAcceptAndSave}>{t('hostIdentity.acceptAndSave')}</Button>
              <Button className="host-identity-card__accept" data-testid="host-identity-accept"
                aria-label={t('hostIdentity.accept')} onClick={onAccept}>{t('hostIdentity.accept')}</Button>
              <Button className="host-identity-card__reject" data-testid="host-identity-reject" aria-label={t('hostIdentity.reject')}
                onClick={onReject}>{t('hostIdentity.reject')}</Button>
            </>
          )}
        </div>
        </Card>
    </div>
  );
}
