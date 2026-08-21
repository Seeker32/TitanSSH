import { ChevronDown, Download, Trash2, Upload } from 'lucide-react';
import { useState } from 'react';
import { formatAppError, translate } from '@/i18n';
import { useLocaleStore } from '@/stores/locale';
import type { TransferTask } from '@/types/sftp';

interface Props {
  tasks: TransferTask[];
  onClear: () => void;
}

/** 渲染关闭会话的近期传输终态；记录仅用于核验，不提供失效 Session 的任务操作。 */
export default function RecentTransfers({ tasks, onClear }: Props) {
  const locale = useLocaleStore((state) => state.locale);
  const [expanded, setExpanded] = useState(false);
  return <section className="recent-transfers">
    <button type="button" className="recent-transfers-header" aria-expanded={expanded}
      onClick={() => setExpanded((value) => !value)}>
      <ChevronDown size={14} className={expanded ? '' : 'recent-transfers-chevron--collapsed'} />
      <span>{translate(locale, 'sftp.recentTransfers')}</span>
      <span className="recent-transfers-count">{tasks.length}</span>
    </button>
    {expanded && <div className="recent-transfers-body">
      {tasks.length === 0
        ? <div className="recent-transfers-empty">{translate(locale, 'sftp.noRecentTransfers')}</div>
        : <>
          <button type="button" className="recent-transfers-clear" onClick={onClear}>
            <Trash2 size={12} />{translate(locale, 'sftp.clearRecentTransfers')}
          </button>
          {tasks.map((task) => <div key={task.taskId} className="recent-transfer-item">
            {task.transferType === 'Download' ? <Download size={13} /> : <Upload size={13} />}
            <span className="recent-transfer-name" title={task.fileName}>{task.fileName}</span>
            <span className={`task-status task-status--${task.status.toLowerCase()}`}>
              {translate(locale, `sftp.${task.status.toLowerCase()}` as Parameters<typeof translate>[1])}
            </span>
            {task.error && <div className="recent-transfer-error">{formatAppError(locale, task.error)}</div>}
          </div>)}
        </>}
    </div>}
  </section>;
}
