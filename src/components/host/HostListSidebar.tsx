import { Input } from 'antd';
import { ChevronDown, Pencil, Plus, Search, Server, Trash2 } from 'lucide-react';
import { useState } from 'react';
import { groupHosts } from '@/stores/host';
import type { HostConfig } from '@/types/host';
import { translate } from '@/i18n';
import { useLocaleStore } from '@/stores/locale';

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
  const locale = useLocaleStore((state) => state.locale);
  const t = (key: Parameters<typeof translate>[1]) => translate(locale, key);
  const searching = searchQuery.trim() !== '';

  return (
    <>
      <div className="sidebar-search-row">
        <Input size="small" value={searchQuery} placeholder={t('host.search')} prefix={<Search size={13} />}
          allowClear onChange={(event) => onSearchChange(event.target.value)} data-testid="host-search" />
        <button type="button" className="sidebar-add-btn" aria-label={t('host.create')} title={t('host.create')} onClick={onCreate}>
          <Plus size={14} />
        </button>
      </div>
      <div className="sidebar-host-list">
        {hosts.length === 0 ? (
          searching ? (
            <div className="sidebar-empty-msg">{t('host.noMatch')}</div>
          ) : (
            <div className="sidebar-empty-msg">
              <Server size={20} className="sidebar-empty-icon" />
              <span>{t('host.empty')}</span>
              <button type="button" className="sidebar-create-btn" onClick={onCreate}>{t('host.createFirst')}</button>
            </div>
          )
        ) : searching ? (
          hosts.map((host) => <HostCard key={host.id} host={host} selected={host.id === selectedHostId}
            onSelect={onSelect} onOpen={onOpen} onEdit={onEditHost} onDelete={onDeleteHost} />)
        ) : (
          groupHosts(hosts).map((group) => (
            <div key={group.name || '__ungrouped__'} className="host-group">
              <GroupHeader locale={locale} name={group.name} count={group.hosts.length}
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
function GroupHeader({ locale, name, count, collapsed, onToggle, onRename, onDelete }: { locale: ReturnType<typeof useLocaleStore.getState>['locale'];
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
          <span className="host-group-name">{name || translate(locale, 'host.ungrouped')}</span>
          <span className="host-group-count">{count}</span>
        </>
      )}
      {name !== '' && !editing && (
        <span className="host-group-actions">
          <button type="button" className="host-group-action" data-testid="group-rename-btn" aria-label={translate(locale, 'host.renameGroup')}
            title={translate(locale, 'host.renameGroup')} onClick={(event) => { event.stopPropagation(); setRenameValue(name); setEditing(true); }}>
            <Pencil size={11} />
          </button>
          <button type="button" className="host-group-action host-group-action--danger" data-testid="group-delete-btn" aria-label={translate(locale, 'host.deleteGroup')}
            title={translate(locale, 'host.deleteGroup')} onClick={(event) => { event.stopPropagation(); onDelete(); }}>
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
  const locale = useLocaleStore((state) => state.locale);
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
        <button type="button" className="host-group-action" data-testid="host-edit-btn" aria-label={translate(locale, 'host.edit')}
          title={translate(locale, 'host.edit')} onClick={(event) => { event.stopPropagation(); onEdit(host.id); }}>
          <Pencil size={11} />
        </button>
        <button type="button" className="host-group-action host-group-action--danger" data-testid="host-delete-btn"
          aria-label={translate(locale, 'host.delete')} title={translate(locale, 'host.delete')} onClick={(event) => { event.stopPropagation(); onDelete(host.id); }}>
          <Trash2 size={11} />
        </button>
      </span>
    </div>
  );
}
