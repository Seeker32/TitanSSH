import { invoke } from '@tauri-apps/api/core';
import { create } from 'zustand';
import type { HostConfig, SaveHostRequest } from '@/types/host';

interface HostState {
  hosts: HostConfig[];
  loading: boolean;
  error: string | null;
  /** 侧栏搜索词，空串表示不过滤 */
  searchQuery: string;
  /** 侧栏当前选中的主机 id，null 表示未选中 */
  selectedHostId: string | null;
  loadHosts: () => Promise<void>;
  saveHost: (request: SaveHostRequest) => Promise<void>;
  deleteHost: (hostId: string) => Promise<void>;
  setSearchQuery: (query: string) => void;
  selectHost: (hostId: string | null) => void;
}

/** 按搜索词过滤主机：空串返回全部，匹配名称或地址，不区分大小写。 */
export function filterHosts(hosts: HostConfig[], query: string): HostConfig[] {
  const keyword = query.trim().toLowerCase();
  if (!keyword) return hosts;
  return hosts.filter((host) =>
    host.name.toLowerCase().includes(keyword) || host.host.toLowerCase().includes(keyword));
}

export const useHostStore = create<HostState>((set) => ({
  hosts: [],
  loading: false,
  error: null,
  searchQuery: '',
  selectedHostId: null,

  /** 加载所有已保存的主机配置列表。 */
  async loadHosts() {
    set({ loading: true, error: null });
    try {
      set({ hosts: await invoke<HostConfig[]>('list_hosts') });
    } catch (error) {
      set({ error: String(error) });
    } finally {
      set({ loading: false });
    }
  },

  /** 保存主机配置；明文凭据仅传给后端安全存储。 */
  async saveHost(request) {
    set({ loading: true, error: null });
    try {
      set({ hosts: await invoke<HostConfig[]>('save_host', { request }) });
    } catch (error) {
      set({ error: String(error) });
      throw error;
    } finally {
      set({ loading: false });
    }
  },

  /** 删除指定主机配置及其安全存储引用。 */
  async deleteHost(hostId) {
    set({ loading: true, error: null });
    try {
      set({ hosts: await invoke<HostConfig[]>('delete_host', { hostId }) });
    } catch (error) {
      set({ error: String(error) });
      throw error;
    } finally {
      set({ loading: false });
    }
  },

  /** 更新侧栏搜索词。 */
  setSearchQuery(query) {
    set({ searchQuery: query });
  },

  /** 更新侧栏选中主机；传 null 清除选中。 */
  selectHost(hostId) {
    set({ selectedHostId: hostId });
  },
}));
