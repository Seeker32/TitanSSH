import { invoke } from '@tauri-apps/api/core';
import { create } from 'zustand';
import type { HostConfig, SaveHostRequest } from '@/types/host';

interface HostState {
  hosts: HostConfig[];
  loading: boolean;
  error: string | null;
  loadHosts: () => Promise<void>;
  saveHost: (request: SaveHostRequest) => Promise<void>;
  deleteHost: (hostId: string) => Promise<void>;
}

export const useHostStore = create<HostState>((set) => ({
  hosts: [],
  loading: false,
  error: null,

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
}));
