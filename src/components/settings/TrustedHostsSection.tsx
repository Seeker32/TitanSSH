import { formatAppError, translate, type TranslationKey } from '@/i18n';
import { useLocaleStore } from '@/stores/locale';
import { useTrustedHostsStore } from '@/stores/trusted-hosts';

interface Props {
  /** 重试读取信任记录（由页面在进入该区域时触发加载，本节只负责渲染状态）。 */
  onRetry: () => void;
}

/** 将精确 endpoint（host + port）渲染为无歧义展示文案：
 *  普通主机显示 `host:port`；IPv6 地址（含冒号）用标准 `[host]:port` 括起。
 *  只影响展示，不做任何归一化，信任归属仍以精确 host + port 为准。 */
export function endpointLabel(host: string, port: number): string {
  return host.includes(':') ? `[${host}]:${port}` : `${host}:${port}`;
}

/** Settings“可信主机”只读区域：展示后端解析出的 endpoint、算法与 SHA-256 指纹。
 *  不提供删除、编辑、导入或导出操作；信任记录随 HostConfig 生命周期自动管理。
 *  空信任存储、读取失败与解析失败是三种不同状态，错误绝不伪装成空列表。
 *  本节是纯渲染投影：加载时机由 HomePage 在每次进入区域时触发，避免
 *  页面级 Modal 复用子节点导致清单停留在旧数据。 */
export default function TrustedHostsSection({ onRetry }: Props) {
  const locale = useLocaleStore((state) => state.locale);
  const status = useTrustedHostsStore((state) => state.status);
  const hosts = useTrustedHostsStore((state) => state.hosts);
  const error = useTrustedHostsStore((state) => state.error);
  const t = (key: TranslationKey) => translate(locale, key);

  if (status === 'idle' || status === 'loading') {
    return (
      <div className="trusted-hosts-state" data-testid="trusted-hosts-loading">
        <span className="spinner" />
        {t('settings.trustedHosts.loading')}
      </div>
    );
  }
  if (status === 'error') {
    return (
      <div className="trusted-hosts-state trusted-hosts-state--error" data-testid="trusted-hosts-error" role="alert">
        <p>{t('settings.trustedHosts.loadFailed')}</p>
        <p className="trusted-hosts-state__detail">{formatAppError(locale, error)}</p>
        <button type="button" data-testid="trusted-hosts-retry" onClick={onRetry}>
          {t('settings.trustedHosts.retry')}
        </button>
      </div>
    );
  }
  if (hosts.length === 0) {
    return (
      <div className="trusted-hosts-state" data-testid="trusted-hosts-empty">
        {t('settings.trustedHosts.empty')}
      </div>
    );
  }
  return (
    <div className="trusted-hosts" data-testid="trusted-hosts-list">
      <p className="trusted-hosts__hint">{t('settings.trustedHosts.readOnlyHint')}</p>
      <table className="trusted-hosts__table">
        <thead>
          <tr>
            <th scope="col">{t('hostIdentity.endpoint')}</th>
            <th scope="col">{t('hostIdentity.algorithm')}</th>
            <th scope="col">{t('hostIdentity.fingerprint')}</th>
          </tr>
        </thead>
        <tbody>
          {hosts.map((host) => (
            <tr key={`${host.host}:${host.port}`} data-testid={`trusted-host-row-${host.host}-${host.port}`}>
              <td className="trusted-hosts__mono">{endpointLabel(host.host, host.port)}</td>
              <td className="trusted-hosts__mono">{host.algorithm}</td>
              <td className="trusted-hosts__mono">{host.fingerprint}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
