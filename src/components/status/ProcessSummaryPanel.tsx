import type { ProcessSortMode } from '@/stores/process';
import { topProcesses } from '@/stores/process';
import type { ProcessSnapshot } from '@/types/process';
import { translate } from '@/i18n';
import { useLocaleStore } from '@/stores/locale';

interface Props {
  snapshot: ProcessSnapshot | null;
  sortMode: ProcessSortMode;
  onSortModeChange: (mode: ProcessSortMode) => void;
}

/** 将进程内存占用格式化为侧栏可读文本。 */
function formatMemory(bytes: number | null) {
  if (bytes === null || bytes <= 0) return '--';
  const units = ['B', 'KB', 'MB', 'GB', 'TB'];
  let value = bytes;
  let index = 0;
  while (value >= 1024 && index < units.length - 1) {
    value /= 1024;
    index += 1;
  }
  return `${value.toFixed(index === 0 ? 0 : 1)} ${units[index]}`;
}

/** 渲染当前会话同一份进程快照派生的 top-5 摘要。 */
export default function ProcessSummaryPanel({ snapshot, sortMode, onSortModeChange }: Props) {
  const locale = useLocaleStore((state) => state.locale);
  const processes = topProcesses(snapshot ?? undefined, sortMode);
  return (
    <section className="process-summary" data-testid="process-summary" aria-label={translate(locale, 'process.title')}>
      <div className="process-summary-header">
        <strong>{translate(locale, 'process.title')}</strong>
        <div className="process-summary-sort" role="group" aria-label={translate(locale, 'process.sort')}>
          <button type="button" aria-pressed={sortMode === 'cpu'} onClick={() => onSortModeChange('cpu')}>{translate(locale, 'process.cpu')}</button>
          <button type="button" aria-pressed={sortMode === 'memory'} onClick={() => onSortModeChange('memory')}>{translate(locale, 'process.memory')}</button>
        </div>
      </div>
      {processes.length === 0 ? <span className="process-summary-empty">{translate(locale, 'process.empty')}</span> : (
        <ol className="process-summary-list">
          {processes.map((process) => (
            <li key={process.pid}>
              <span className="process-summary-command" title={process.commandLine}>{process.command}</span>
              <span className="process-summary-pid">PID {process.pid}</span>
              <span>{process.cpuPercent === null ? '--' : `${process.cpuPercent.toFixed(1)}%`}</span>
              <span>{formatMemory(process.memoryBytes)}</span>
            </li>
          ))}
        </ol>
      )}
    </section>
  );
}
