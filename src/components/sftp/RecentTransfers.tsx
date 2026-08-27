import { Download, Trash2, Upload } from 'lucide-react';
import { useState } from 'react';
import { Badge, Button, Collapse, Empty, Tag } from 'antd';
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
  const statusColors = { Pending: 'gold', Running: 'blue', Done: 'green', Failed: 'red', Cancelled: 'default' } as const;
  const content = tasks.length === 0
    ? <div className="recent-transfers-empty"><Empty image={Empty.PRESENTED_IMAGE_SIMPLE} description={translate(locale, 'sftp.noRecentTransfers')} /></div>
    : <div className="recent-transfers-body">
      <Button type="text" size="small" className="recent-transfers-clear" onClick={onClear} icon={<Trash2 size={12} />}>
        {translate(locale, 'sftp.clearRecentTransfers')}
      </Button>
      {tasks.map((task) => <div key={task.taskId} className="recent-transfer-item">
        {task.transferType === 'Download' ? <Download size={13} /> : <Upload size={13} />}
        <span className="recent-transfer-name" title={task.fileName}>{task.fileName}</span>
        <Tag variant="filled" color={statusColors[task.status]} className={`task-status task-status--${task.status.toLowerCase()}`}>
          {translate(locale, `sftp.${task.status.toLowerCase()}` as Parameters<typeof translate>[1])}
        </Tag>
        {task.error && <div className="recent-transfer-error">{formatAppError(locale, task.error)}</div>}
      </div>)}
    </div>;
  return <section className="recent-transfers">
    <Collapse ghost destroyOnHidden activeKey={expanded ? ['recent'] : []} onChange={(keys) => setExpanded(keys.includes('recent'))}
      items={[{ key: 'recent', label: <><span>{translate(locale, 'sftp.recentTransfers')}</span><Badge size="small" count={tasks.length} showZero /></>, children: content }]} />
  </section>;
}
