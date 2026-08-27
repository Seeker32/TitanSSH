import { Button, Card, Empty } from 'antd';
import type { ProcessSortMode } from '@/stores/process';
import { topProcesses } from '@/stores/process';
import type { ProcessSnapshot } from '@/types/process';
import { translate } from '@/i18n';
import { useLocaleStore } from '@/stores/locale';
import type { MouseEvent, KeyboardEvent } from 'react';

interface Props {
  snapshot: ProcessSnapshot | null;
  sortMode: ProcessSortMode;
  onSortModeChange: (mode: ProcessSortMode) => void;
  onOpenProcess?: () => void;
}

/** 将进程内存占用格式化为侧栏可读文本。 */
export function formatMemory(bytes: number | null) {
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
export default function ProcessSummaryPanel({ snapshot, sortMode, onSortModeChange, onOpenProcess }: Props) {
  const locale = useLocaleStore((state) => state.locale);
  const processes = topProcesses(snapshot ?? undefined, sortMode);
  /** 点击 top-5 列表打开全量进程标签；排序按钮只执行自身操作。 */
  function openFromPanel(event: MouseEvent<HTMLElement>) {
    if (onOpenProcess && !(event.target as HTMLElement).closest('button')) onOpenProcess();
  }
  /** 为可点击摘要面板提供键盘等价操作。 */
  function openFromKeyboard(event: KeyboardEvent<HTMLElement>) {
    if (onOpenProcess && (event.key === 'Enter' || event.key === ' ')) {
      event.preventDefault();
      onOpenProcess();
    }
  }
  return (
    <section className="process-summary" data-testid="process-summary" aria-label={translate(locale, 'process.title')}
      onClick={onOpenProcess ? openFromPanel : undefined} onKeyDown={onOpenProcess ? openFromKeyboard : undefined} tabIndex={onOpenProcess ? 0 : undefined}>
      <Card size="small" variant="borderless">
        <div className="process-summary-header">
          <strong>{translate(locale, 'process.title')}</strong>
          {onOpenProcess && <Button type="link" size="small" className="process-summary-open" onClick={onOpenProcess}>{translate(locale, 'process.open')}</Button>}
          <div className="process-summary-sort" role="group" aria-label={translate(locale, 'process.sort')}>
            <Button size="small" type={sortMode === 'cpu' ? 'primary' : 'default'} aria-label={translate(locale, 'process.cpu')} aria-pressed={sortMode === 'cpu'} onClick={() => onSortModeChange('cpu')}>{translate(locale, 'process.cpu')}</Button>
            <Button size="small" type={sortMode === 'memory' ? 'primary' : 'default'} aria-label={translate(locale, 'process.memory')} aria-pressed={sortMode === 'memory'} onClick={() => onSortModeChange('memory')}>{translate(locale, 'process.memory')}</Button>
          </div>
        </div>
        {processes.length === 0 ? <div className="process-summary-empty"><Empty image={Empty.PRESENTED_IMAGE_SIMPLE} description={translate(locale, 'process.empty')} /></div> : (
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
      </Card>
    </section>
  );
}
