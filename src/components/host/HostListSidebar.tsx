import { Input } from 'antd';
import { Plus, Search, Server } from 'lucide-react';
import type { HostConfig } from '@/types/host';

interface Props {
  /** 已按搜索词过滤的主机列表 */
  hosts: HostConfig[];
  searchQuery: string;
  selectedHostId: string | null;
  onSearchChange: (query: string) => void;
  onSelect: (hostId: string | null) => void;
  onOpen: (hostId: string) => void;
  onCreate: () => void;
}

/** 渲染侧栏常驻主机列表：搜索行 + 平铺主机卡片（分组在后续迭代）。 */
export default function HostListSidebar({ hosts, searchQuery, selectedHostId, onSearchChange, onSelect, onOpen, onCreate }: Props) {
  return (
    <>
      <div className="sidebar-search-row">
        <Input size="small" value={searchQuery} placeholder="搜索主机…" prefix={<Search size={13} />}
          allowClear onChange={(event) => onSearchChange(event.target.value)} data-testid="host-search" />
        <button type="button" className="sidebar-add-btn" aria-label="新建主机" title="新建主机" onClick={onCreate}>
          <Plus size={14} />
        </button>
      </div>
      <div className="sidebar-host-list">
        {hosts.length === 0 ? (
          searchQuery.trim() ? (
            <div className="sidebar-empty-msg">未找到匹配的主机</div>
          ) : (
            <div className="sidebar-empty-msg">
              <Server size={20} className="sidebar-empty-icon" />
              <span>暂无主机，点击下方按钮添加第一个 SSH 连接</span>
              <button type="button" className="sidebar-create-btn" onClick={onCreate}>新建第一个主机</button>
            </div>
          )
        ) : (
          hosts.map((host) => (
            <div key={host.id} role="button" tabIndex={0} data-testid={`host-card-${host.id}`}
              className={`host-card${host.id === selectedHostId ? ' host-card--selected' : ''}`}
              onClick={() => onSelect(host.id)}
              onDoubleClick={() => onOpen(host.id)}
              onKeyDown={(event) => event.key === 'Enter' && onOpen(host.id)}>
              <Server size={14} className="host-card-icon" />
              <div className="host-card-copy">
                <span className="host-card-name">{host.name || host.host}</span>
                <span className="host-card-address">{host.username}@{host.host}:{host.port}</span>
              </div>
            </div>
          ))
        )}
      </div>
    </>
  );
}
