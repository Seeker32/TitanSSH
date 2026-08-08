import { Download, RotateCcw, Upload, X } from 'lucide-react';
import type { SftpTaskStatus, TransferTask } from '@/types/sftp';

interface Props {
  tasks: Map<string, TransferTask>;
  onCancel: (taskId: string) => void;
  onRetry: (task: TransferTask) => void;
}

/** 计算传输进度百分比。 */
function progressPct(task: TransferTask) {
  return task.totalBytes === 0 ? 0 : Math.round((task.transferredBytes / task.totalBytes) * 100);
}

/** 格式化传输速度。 */
function formatSpeed(bytesPerSecond: number) {
  if (bytesPerSecond === 0) return '—';
  if (bytesPerSecond < 1024) return `${bytesPerSecond} B/s`;
  if (bytesPerSecond < 1024 * 1024) return `${(bytesPerSecond / 1024).toFixed(1)} KB/s`;
  return `${(bytesPerSecond / 1024 / 1024).toFixed(1)} MB/s`;
}

/** 将任务状态转换为中文标签。 */
function taskStatusLabel(status: SftpTaskStatus) {
  return ({ Pending: '等待中', Running: '传输中', Done: '完成', Failed: '失败', Cancelled: '已取消' })[status];
}

/** 渲染当前会话的传输任务列表。 */
export default function TransferQueue({ tasks, onCancel, onRetry }: Props) {
  if (tasks.size === 0) return <div className="empty-msg">暂无传输任务</div>;
  return <div className="transfer-queue">{[...tasks.values()].map((task) => {
    const active = task.status === 'Pending' || task.status === 'Running';
    const percent = progressPct(task);
    return <div key={task.taskId} className="task-item">
      <div className="task-top">
        <span className="task-direction">{task.transferType === 'Download' ? <Download size={13} /> : <Upload size={13} />}</span>
        <span className="task-name" title={task.fileName}>{task.fileName}</span>
        <span className={`task-status task-status--${task.status.toLowerCase()}`}>{taskStatusLabel(task.status)}</span>
        {active && <button data-testid="cancel-btn" className="task-btn" title="取消" onClick={() => onCancel(task.taskId)}><X size={12} /></button>}
        {(task.status === 'Failed' || task.status === 'Cancelled')
          && <button data-testid="retry-btn" className="task-btn" title="重新发起" onClick={() => onRetry(task)}><RotateCcw size={12} /></button>}
      </div>
      <div className="progress-bar" data-testid="progress-bar">
        <div className={`progress-fill ${task.status === 'Done' ? 'progress-fill--done' : ''}`}
          data-testid="progress-fill" style={{ width: `${percent}%` }} />
      </div>
      <div className="task-meta"><span>{formatSpeed(task.speedBps)}</span><span>{percent}%</span></div>
      {task.status === 'Failed' && task.errorMessage && <div className="task-error">{task.errorMessage}</div>}
    </div>;
  })}</div>;
}
