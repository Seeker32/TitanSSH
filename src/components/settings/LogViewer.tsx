import { useEffect, useRef } from 'react';
import { formatAppError, translate, type TranslationKey } from '@/i18n';
import { useLocaleStore } from '@/stores/locale';
import { useLogsStore } from '@/stores/logs';

/** 查看器轮询间隔：打开期间每 2 秒拉取一次最近日志。 */
const LOG_POLL_INTERVAL_MS = 2_000;

/** Settings“日志”分区内嵌查看器：挂载即加载并每 2 秒轮询（卸载即停止），
 *  导出由后端弹出保存对话框并复制日志文件（目标路径不经 IPC 边界）。
 *  日志行为纯文本展示，不做解析或着色。 */
export default function LogViewer() {
  const locale = useLocaleStore((state) => state.locale);
  const lines = useLogsStore((state) => state.lines);
  const loadError = useLogsStore((state) => state.loadError);
  const exportError = useLogsStore((state) => state.exportError);
  const t = (key: TranslationKey) => translate(locale, key);

  /** 导出进行中标记：防止快速重复点击打开多个保存对话框/并发导出。 */
  const exportingRef = useRef(false);

  useEffect(() => {
    void useLogsStore.getState().load();
    const timer = setInterval(() => void useLogsStore.getState().load(), LOG_POLL_INTERVAL_MS);
    return () => clearInterval(timer);
  }, []);

  /** 发起日志导出：保存对话框由后端弹出，目标路径不经 IPC 边界；
   *  导出进行中忽略重复点击。store 内部捕获错误，调用方无需处理拒绝。 */
  async function handleExport() {
    if (exportingRef.current) return;
    exportingRef.current = true;
    try {
      await useLogsStore.getState().exportLogs();
    } finally {
      exportingRef.current = false;
    }
  }

  return (
    <div className="log-viewer" data-testid="log-viewer">
      <div className="log-viewer__toolbar">
        <button type="button" data-testid="log-refresh-btn" onClick={() => void useLogsStore.getState().load()}>
          {t('settings.logRefresh')}
        </button>
        <button type="button" data-testid="log-export-btn" onClick={() => void handleExport()}>
          {t('settings.logExport')}
        </button>
      </div>
      {loadError && (
        <p className="log-viewer__error" data-testid="log-viewer-load-error" role="alert">
          {t('settings.logLoadFailed')}: {formatAppError(locale, loadError)}
        </p>
      )}
      {exportError && (
        <p className="log-viewer__error" data-testid="log-viewer-export-error" role="alert">
          {t('settings.logExportFailed')}: {formatAppError(locale, exportError)}
        </p>
      )}
      {lines.length === 0 && !loadError ? (
        <p className="log-viewer__empty" data-testid="log-viewer-empty">{t('settings.logEmpty')}</p>
      ) : (
        <pre className="log-viewer__lines" data-testid="log-viewer-lines">{lines.join('\n')}</pre>
      )}
    </div>
  );
}
