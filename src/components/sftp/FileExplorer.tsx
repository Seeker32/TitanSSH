import { FileText, Folder } from 'lucide-react';
import type { KeyboardEvent, MouseEvent } from 'react';
import type { RemoteEntry, SftpSessionState } from '@/types/sftp';

interface Props {
  state: SftpSessionState;
  onNavigate: (path: string) => void;
  onSelect: (path: string) => void;
  onUpload: () => void;
  onDownload: (paths: string[]) => void;
}

/** 将路径拆分为可点击的面包屑。 */
function pathSegments(path: string) {
  const parts = path.split('/').filter(Boolean);
  return parts.map((label, index) => ({ label, path: `/${parts.slice(0, index + 1).join('/')}` }));
}

/** 格式化远程文件大小。 */
function formatSize(bytes: number) {
  if (bytes === 0) return '—';
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}

/** 格式化远程文件修改时间。 */
function formatDate(timestamp: number) {
  return timestamp ? new Date(timestamp).toLocaleDateString('zh-CN') : '';
}

/** 渲染远程路径与文件列表。 */
export default function FileExplorer({ state, onNavigate, onSelect, onUpload, onDownload }: Props) {
  const entries = [...state.entries].sort((a, b) => Number(b.isDir) - Number(a.isDir));

  /** 单击文件时切换选择。 */
  function handleClick(entry: RemoteEntry) {
    if (!entry.isDir) onSelect(entry.path);
  }

  /** 双击目录进入，双击文件下载。 */
  function handleOpen(entry: RemoteEntry) {
    entry.isDir ? onNavigate(entry.path) : onDownload([entry.path]);
  }

  /** 支持键盘 Enter 打开条目。 */
  function handleKey(event: KeyboardEvent, entry: RemoteEntry) {
    if (event.key === 'Enter') handleOpen(entry);
  }

  /** 避免双击文件前的第二次单击改变父级行为。 */
  function handleDoubleClick(event: MouseEvent, entry: RemoteEntry) {
    event.preventDefault();
    handleOpen(entry);
  }

  return (
    <div className="file-explorer">
      <div className="path-bar">
        <button className="path-seg path-seg--root" onClick={() => onNavigate('/')}>/</button>
        {pathSegments(state.currentPath).map((segment) => (
          <span key={segment.path}><span className="path-sep">›</span>
            <button className="path-seg" onClick={() => onNavigate(segment.path)}>{segment.label}</button></span>
        ))}
        <span className="path-actions">
          <button onClick={onUpload}>上传</button>
          <button disabled={state.selectedPaths.size === 0} onClick={() => onDownload([...state.selectedPaths])}>下载</button>
        </span>
      </div>
      {state.loading ? <div className="state-msg">加载中...</div>
        : state.error ? <div className="state-msg state-msg--error">{state.error}</div>
          : entries.length === 0 ? <div className="state-msg">空目录</div>
            : <div className="file-list" role="rowgroup">{entries.map((entry) => (
              <div key={entry.path} data-testid="file-row" className={`file-row ${state.selectedPaths.has(entry.path) ? 'file-row--selected' : ''}`}
                role="row" tabIndex={0} onClick={() => handleClick(entry)} onDoubleClick={(event) => handleDoubleClick(event, entry)}
                onKeyDown={(event) => handleKey(event, entry)}>
                <span className={`file-icon ${entry.isDir ? 'file-icon--dir' : ''}`}>
                  {entry.isDir ? <Folder size={14} /> : <FileText size={14} />}
                </span>
                <span className={`file-name ${entry.isDir ? 'file-name--dir' : ''}`}>{entry.name}</span>
                <span className="file-size">{formatSize(entry.size)}</span>
                <span className="file-date">{formatDate(entry.modifiedAt)}</span>
              </div>
            ))}</div>}
    </div>
  );
}
