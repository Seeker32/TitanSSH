import { Terminal } from 'lucide-react';

interface Props {
  onCreateHost: () => void;
}

/** 无会话时主区空态页：引导文案与新建主机入口。 */
export default function EmptyState({ onCreateHost }: Props) {
  return (
    <div className="empty-state">
      <Terminal size={40} className="empty-state-icon" />
      <p className="empty-state-title">选择左侧主机开始连接</p>
      <p className="empty-state-hint">双击主机卡片打开 SSH 会话，或先添加一台主机</p>
      <button type="button" className="sidebar-create-btn" onClick={onCreateHost}>新建主机</button>
    </div>
  );
}
