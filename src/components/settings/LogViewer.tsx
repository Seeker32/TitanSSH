import { useEffect } from 'react';
import { save as saveFileDialog } from '@tauri-apps/plugin-dialog';
import { formatAppError, translate, type TranslationKey } from '@/i18n';
import { useLocaleStore } from '@/stores/locale';
import { useLogsStore } from '@/stores/logs';

/** 查看器轮询间隔：打开期间每 2 秒拉取一次最近日志。 */
const LOG_POLL_INTERVAL_MS = 2_000;

/** 导出默认文件名：titanssh-<yyyy-mm-dd_hh-mm-ss>.log，便于按时间归档。 */
function defaultExportName(): string {
  const now = new Date();
  const pad = (value: number) => String(value).padStart(2, '0');
  return `titanssh-${now.getFullYear()}-${pad(now.getMonth() + 1)}-${pad(now.getDate())}_${pad(now.getHours())}-${pad(now.getMinutes())}-${pad(now.getSeconds())}.log`;
}

/** Settings“日志”分区内嵌查看器：挂载即加载并每 2 秒轮询（卸载即停止），
 *  导出经原生保存对话框复制后端日志文件；用户取消不发起任何 invoke。
 *  日志行为纯文本展示，不做解析或着色。 */
export default function LogViewer() {
  const locale = useLocaleStore((state) => state.locale);
  const lines = useLogsStore((state) => state.lines);
  const loadError = useLogsStore((state) => state.loadError);
  const exportError = useLogsStore((state) => state.exportError);
  const t = (key: TranslationKey) => translate(locale, key);

  useEffect(() => {
    void useLogsStore.getState().load();
    const timer = setInterval(() => void useLogsStore.getState().load(), LOG_POLL_INTERVAL_MS);
    return () => clearInterval(timer);
  }, []);

  /** 弹出保存对话框并把后端日志文件复制到所选路径；用户取消则不 invoke。 */
  async function exportLogs() {
    const path = await saveFileDialog({ defaultPath: defaultExportName() });
    if (path) await useLogsStore.getState().export(path);
  }

  return (
    <div className="log-viewer" data-testid="log-viewer">
      <div className="log-viewer__toolbar">
        <button type="button" data-testid="log-refresh-btn" onClick={() => void useLogsStore.getState().load()}>
          {t('settings.logRefresh')}
        </button>
        <button type="button" data-testid="log-export-btn" onClick={() => void exportLogs()}>
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
