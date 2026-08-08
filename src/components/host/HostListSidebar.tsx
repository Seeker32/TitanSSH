import { Input } from 'antd';
import { ChevronDown, Pencil, Plus, Search, Server, Trash2 } from 'lucide-react';
import { useState } from 'react';
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
  onRenameGroup: (oldName: string, newName: string) => void;
  onDeleteGroup: (name: string) => void;
  onEditHost: (hostId: string) => void;
  onDeleteHost: (hostId: string) => void;
  onSearchChange: (query: string) => void;
  onSelect: (hostId: string | null) => void;
  onOpen: (hostId: string) => void;
  onCreate: () => void;
}

/** 渲染侧栏常驻主机列表：搜索行 + 分组折叠列表。搜索时切换为平铺列表。 */
export default function HostListSidebar({ hosts, searchQuery, selectedHostId, collapsedGroups,
  onToggleGroup, onRenameGroup, onDeleteGroup, onEditHost, onDeleteHost,
  onSearchChange, onSelect, onOpen, onCreate }: Props) {
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
            onSelect={onSelect} onOpen={onOpen} onEdit={onEditHost} onDelete={onDeleteHost} />)
        ) : (
          groupHosts(hosts).map((group) => (
            <div key={group.name || '__ungrouped__'} className="host-group">
              <GroupHeader name={group.name} count={group.hosts.length}
                collapsed={collapsedGroups.includes(group.name)}
                onToggle={() => onToggleGroup(group.name)}
                onRename={(newName) => onRenameGroup(group.name, newName)}
                onDelete={() => onDeleteGroup(group.name)} />
              {!collapsedGroups.includes(group.name) && (
                <div className="host-group-body">
                  {group.hosts.map((host) => <HostCard key={host.id} host={host} selected={host.id === selectedHostId}
                    onSelect={onSelect} onOpen={onOpen} onEdit={onEditHost} onDelete={onDeleteHost} />)}
                </div>
              )}
            </div>
          ))
        )}
      </div>
    </>
  );
}

/** 分组头：折叠切换 + hover 重命名/删除；未分组不提供操作。 */
function GroupHeader({ name, count, collapsed, onToggle, onRename, onDelete }: {
  name: string; count: number; collapsed: boolean;
  onToggle: () => void; onRename: (newName: string) => void; onDelete: () => void;
}) {
  const [editing, setEditing] = useState(false);
  const [renameValue, setRenameValue] = useState(name);

  /** 提交行内重命名并退出编辑态。 */
  function commitRename() {
    onRename(renameValue);
    setEditing(false);
  }

  /** 取消行内重命名。 */
  function cancelRename() {
    setEditing(false);
    setRenameValue(name);
  }

  return (
    <div role="button" tabIndex={0} data-testid={`group-header-${name || 'ungrouped'}`}
      className={`host-group-header${collapsed ? ' host-group-header--collapsed' : ''}`}
      onClick={editing ? undefined : onToggle}
      onKeyDown={(event) => event.key === 'Enter' && !editing && onToggle()}>
      <ChevronDown size={12} className="host-group-chevron" />
      {editing ? (
        <Input size="small" autoFocus value={renameValue} data-testid="group-rename-input"
          onClick={(event) => event.stopPropagation()}
          onChange={(event) => setRenameValue(event.target.value)}
          onPressEnter={commitRename}
          onKeyDown={(event) => event.key === 'Escape' && cancelRename()} />
      ) : (
        <>
          <span className="host-group-name">{name || '未分组'}</span>
          <span className="host-group-count">{count}</span>
        </>
      )}
      {name !== '' && !editing && (
        <span className="host-group-actions">
          <button type="button" className="host-group-action" data-testid="group-rename-btn" aria-label="重命名分组"
            title="重命名分组" onClick={(event) => { event.stopPropagation(); setRenameValue(name); setEditing(true); }}>
            <Pencil size={11} />
          </button>
          <button type="button" className="host-group-action host-group-action--danger" data-testid="group-delete-btn" aria-label="删除分组"
            title="删除分组（主机归入未分组）" onClick={(event) => { event.stopPropagation(); onDelete(); }}>
            <Trash2 size={11} />
          </button>
        </span>
      )}
    </div>
  );
}

/** 单个主机卡片：单击选中，双击连接；hover 提供编辑/删除。 */
function HostCard({ host, selected, onSelect, onOpen, onEdit, onDelete }: {
  host: HostConfig; selected: boolean; onSelect: (hostId: string | null) => void; onOpen: (hostId: string) => void;
  onEdit: (hostId: string) => void; onDelete: (hostId: string) => void;
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
      <span className="host-card-actions">
        <button type="button" className="host-group-action" data-testid="host-edit-btn" aria-label="编辑主机"
          title="编辑主机" onClick={(event) => { event.stopPropagation(); onEdit(host.id); }}>
          <Pencil size={11} />
        </button>
        <button type="button" className="host-group-action host-group-action--danger" data-testid="host-delete-btn"
          aria-label="删除主机" title="删除主机" onClick={(event) => { event.stopPropagation(); onDelete(host.id); }}>
          <Trash2 size={11} />
        </button>
      </span>
    </div>
  );
}
