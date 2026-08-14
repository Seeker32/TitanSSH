import { Download, RotateCcw, Trash2, Upload, X } from 'lucide-react';
import type { AppErrorInfo } from '@/i18n';
import { isTerminalStatus, type SftpTaskStatus, type TransferTask } from '@/types/sftp';
import { formatAppError, translate } from '@/i18n';
import { useLocaleStore } from '@/stores/locale';

interface Props {
  tasks: Map<string, TransferTask>;
  /** 任务行级操作错误（取消/重试 invoke 拒绝）；键为 taskId */
  actionErrors: Map<string, AppErrorInfo>;
  onCancel: (taskId: string) => void;
  onRetry: (task: TransferTask) => void;
  /** 对单个冲突文件确认覆盖：仅 Failed + SftpTargetExists 任务行出现入口 */
  onOverwrite: (task: TransferTask) => void;
  onClearTerminal: () => void;
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
function taskStatusLabel(status: SftpTaskStatus, locale: ReturnType<typeof useLocaleStore.getState>['locale']) {
  return translate(locale, `sftp.${status.toLowerCase()}` as Parameters<typeof translate>[1]);
}

/** 渲染当前会话的传输任务列表（createdAt 最新优先）。 */
export default function TransferQueue({ tasks, actionErrors, onCancel, onRetry, onOverwrite, onClearTerminal }: Props) {
  const locale = useLocaleStore((state) => state.locale);
  if (tasks.size === 0) return <div className="empty-msg">{translate(locale, 'sftp.noTasks')}</div>;
  const sorted = [...tasks.values()].sort((a, b) => b.createdAt - a.createdAt);
  const hasTerminal = sorted.some((task) => isTerminalStatus(task.status));
  return <div className="transfer-queue">
    {hasTerminal && <div className="transfer-queue-actions">
      <button type="button" data-testid="clear-terminal-btn" className="task-btn"
        title={translate(locale, 'sftp.clearTerminal')} onClick={onClearTerminal}>
        <Trash2 size={12} />{translate(locale, 'sftp.clearTerminal')}
      </button>
    </div>}
    {sorted.map((task) => {
      const active = task.status === 'Pending' || task.status === 'Running';
      const percent = progressPct(task);
      const actionError = actionErrors.get(task.taskId);
      return <div key={task.taskId} className="task-item">
        <div className="task-top">
          <span className="task-direction">{task.transferType === 'Download' ? <Download size={13} /> : <Upload size={13} />}</span>
          <span className="task-name" title={task.fileName}>{task.fileName}</span>
          <span className={`task-status task-status--${task.status.toLowerCase()}`}>{taskStatusLabel(task.status, locale)}</span>
          {active && <button data-testid="cancel-btn" className="task-btn" title={translate(locale, 'sftp.cancel')} onClick={() => onCancel(task.taskId)}><X size={12} /></button>}
          {(task.status === 'Failed' || task.status === 'Cancelled')
            && <button data-testid="retry-btn" className="task-btn" title={translate(locale, 'sftp.retry')} onClick={() => onRetry(task)}><RotateCcw size={12} /></button>}
          {task.status === 'Failed' && task.error?.code === 'SftpTargetExists'
            && <button data-testid="overwrite-btn" className="task-btn"
              title={translate(locale, task.transferType === 'Download' ? 'sftp.overwriteDownload' : 'sftp.overwriteUpload')}
              onClick={() => onOverwrite(task)}>{translate(locale, task.transferType === 'Download' ? 'sftp.overwriteDownload' : 'sftp.overwriteUpload')}</button>}
        </div>
        <div className="progress-bar" data-testid="progress-bar">
          <div className={`progress-fill ${task.status === 'Done' ? 'progress-fill--done' : ''}`}
            data-testid="progress-fill" style={{ width: `${percent}%` }} />
        </div>
        <div className="task-meta"><span>{formatSpeed(task.speedBps)}</span><span>{percent}%</span></div>
        {task.error && <div className="task-error">{formatAppError(locale, task.error)}</div>}
        {actionError && <div data-testid="task-action-error" className="task-error">{formatAppError(locale, actionError)}</div>}
      </div>;
    })}</div>;
}
