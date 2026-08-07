import { Input } from 'antd';
import { ChevronDown, Plus, Search, Server } from 'lucide-react';
import { groupHosts } from '@/stores/host';
import type { HostConfig } from '@/types/host';

interface Props {
  /** 已按搜索词过滤的主机列表 */
  hosts: HostConfig[];
  searchQuery: string;
  selectedHostId: string | null;
  /** 折叠的分组名列表（空串 = "未分组"） */
  collapsedGroups: string[];
  onToggleGroup: (name: string) => void;
  onSearchChange: (query: string) => void;
  onSelect: (hostId: string | null) => void;
  onOpen: (hostId: string) => void;
  onCreate: () => void;
}

/** 渲染侧栏常驻主机列表：搜索行 + 分组折叠列表。搜索时切换为平铺列表。 */
export default function HostListSidebar({ hosts, searchQuery, selectedHostId, collapsedGroups,
  onToggleGroup, onSearchChange, onSelect, onOpen, onCreate }: Props) {
  const searching = searchQuery.trim() !== '';

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
          searching ? (
            <div className="sidebar-empty-msg">未找到匹配的主机</div>
          ) : (
            <div className="sidebar-empty-msg">
              <Server size={20} className="sidebar-empty-icon" />
              <span>暂无主机，点击下方按钮添加第一个 SSH 连接</span>
              <button type="button" className="sidebar-create-btn" onClick={onCreate}>新建第一个主机</button>
            </div>
          )
        ) : searching ? (
          hosts.map((host) => <HostCard key={host.id} host={host} selected={host.id === selectedHostId}
            onSelect={onSelect} onOpen={onOpen} />)
        ) : (
          groupHosts(hosts).map((group) => (
            <div key={group.name || '__ungrouped__'} className="host-group">
              <div role="button" tabIndex={0} data-testid={`group-header-${group.name || 'ungrouped'}`}
                className={`host-group-header${collapsedGroups.includes(group.name) ? ' host-group-header--collapsed' : ''}`}
                onClick={() => onToggleGroup(group.name)}
                onKeyDown={(event) => event.key === 'Enter' && onToggleGroup(group.name)}>
                <ChevronDown size={12} className="host-group-chevron" />
                <span className="host-group-name">{group.name || '未分组'}</span>
                <span className="host-group-count">{group.hosts.length}</span>
              </div>
              {!collapsedGroups.includes(group.name) && (
                <div className="host-group-body">
                  {group.hosts.map((host) => <HostCard key={host.id} host={host} selected={host.id === selectedHostId}
                    onSelect={onSelect} onOpen={onOpen} />)}
                </div>
              )}
            </div>
          ))
        )}
      </div>
    </>
  );
}

/** 单个主机卡片：单击选中，双击连接。 */
function HostCard({ host, selected, onSelect, onOpen }: {
  host: HostConfig; selected: boolean; onSelect: (hostId: string | null) => void; onOpen: (hostId: string) => void;
}) {
  return (
    <div role="button" tabIndex={0} data-testid={`host-card-${host.id}`}
      className={`host-card${selected ? ' host-card--selected' : ''}`}
      onClick={() => onSelect(host.id)}
      onDoubleClick={() => onOpen(host.id)}
      onKeyDown={(event) => event.key === 'Enter' && onOpen(host.id)}>
      <Server size={14} className="host-card-icon" />
      <div className="host-card-copy">
        <span className="host-card-name">{host.name || host.host}</span>
        <span className="host-card-address">{host.username}@{host.host}:{host.port}</span>
      </div>
    </div>
  );
}
