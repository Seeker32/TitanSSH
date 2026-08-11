import { useEffect, useRef, useState } from 'react';
import type { PointerEvent as ReactPointerEvent } from 'react';
import type { SftpSessionState, TransferTask } from '@/types/sftp';
import FileExplorer from './FileExplorer';
import TransferQueue from './TransferQueue';
import { translate } from '@/i18n';
import { useLocaleStore } from '@/stores/locale';

interface Props {
  sessionId: string;
  state: SftpSessionState | null;
  onNavigate: (sessionId: string, path: string) => void;
  onSelect: (sessionId: string, path: string) => void;
  onUpload: (sessionId: string, remotePath: string) => void;
  onDownload: (sessionId: string, paths: string[]) => void;
  onCancel: (taskId: string) => void;
  onRetry: (task: TransferTask) => void;
}

const MIN_HEIGHT = 120;
const MAX_HEIGHT = 600;

/** 限制 SFTP 面板高度。 */
function clampHeight(height: number) {
  return Math.max(MIN_HEIGHT, Math.min(MAX_HEIGHT, height));
}

/** 渲染可调整高度的文件浏览器与传输队列。 */
export default function SftpPanel(props: Props) {
  const locale = useLocaleStore((state) => state.locale);
  const [tab, setTab] = useState<'explorer' | 'queue'>('explorer');
  const [height, setHeight] = useState(280);
  const dragging = useRef(false);
  const startY = useRef(0);
  const startHeight = useRef(0);

  useEffect(() => {
    /** 拖动时更新 SFTP 面板高度。 */
    function move(event: PointerEvent) {
      if (dragging.current) setHeight(clampHeight(startHeight.current + startY.current - event.clientY));
    }
    /** 结束面板高度拖动。 */
    function stop() {
      dragging.current = false;
      document.body.classList.remove('sftp-resizing');
    }
    window.addEventListener('pointermove', move);
    window.addEventListener('pointerup', stop);
    return () => {
      window.removeEventListener('pointermove', move);
      window.removeEventListener('pointerup', stop);
      stop();
    };
  }, []);

  /** 启动 SFTP 面板高度拖动：阻止默认行为防止拖动过程中文本被选中。 */
  function startResize(event: ReactPointerEvent) {
    event.preventDefault();
    dragging.current = true;
    startY.current = event.clientY;
    startHeight.current = height;
    document.body.classList.add('sftp-resizing');
  }

  return <div className="sftp-panel" style={{ height }}>
    <div data-testid="sftp-resizer" className="sftp-resizer" role="separator" aria-orientation="horizontal" onPointerDown={startResize} />
    <div className="sftp-header">
      <button data-testid="tab-explorer" className={`sftp-tab ${tab === 'explorer' ? 'sftp-tab--active' : ''}`}
        onClick={() => setTab('explorer')}>{translate(locale, 'sftp.explorer')}</button>
      <button data-testid="tab-queue" className={`sftp-tab ${tab === 'queue' ? 'sftp-tab--active' : ''}`}
        onClick={() => setTab('queue')}>{translate(locale, 'sftp.queue')}</button>
    </div>
    {!props.state ? <div className="sftp-placeholder">{translate(locale, 'sftp.selectSession')}</div>
      : tab === 'explorer' ? <FileExplorer state={props.state}
        onNavigate={(path) => props.onNavigate(props.sessionId, path)}
        onSelect={(path) => props.onSelect(props.sessionId, path)}
        onUpload={() => props.onUpload(props.sessionId, props.state!.currentPath)}
        onDownload={(paths) => props.onDownload(props.sessionId, paths)} />
        : <TransferQueue tasks={props.state.tasks} onCancel={props.onCancel} onRetry={props.onRetry} />}
  </div>;
}
