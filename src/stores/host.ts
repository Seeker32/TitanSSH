import { defineStore } from 'pinia';
import { invoke } from '@tauri-apps/api/core';
import { ref } from 'vue';
import type { HostConfig, SaveHostRequest } from '@/types/host';

export const useHostStore = defineStore('host', () => {
  const hosts = ref<HostConfig[]>([]);
  const loading = ref(false);
  const error = ref<string | null>(null);

  /** 加载所有已保存的主机配置列表 */
  async function loadHosts() {
    loading.value = true;
    error.value = null;
    try {
      hosts.value = await invoke<HostConfig[]>('list_hosts');
    } catch (err) {
      error.value = String(err);
    } finally {
      loading.value = false;
    }
  }

  /** 保存主机配置，接收含明文凭据的 SaveHostRequest，后端负责安全存储 */
  async function saveHost(request: SaveHostRequest) {
    loading.value = true;
    error.value = null;
    try {
      hosts.value = await invoke<HostConfig[]>('save_host', { request });
    } catch (err) {
      error.value = String(err);
      throw err;
    } finally {
      loading.value = false;
    }
  }

  /** 删除指定主机配置，后端同步清理安全存储中的凭据 */
  async function deleteHost(hostId: string) {
    loading.value = true;
    error.value = null;
    try {
      hosts.value = await invoke<HostConfig[]>('delete_host', { hostId });
    } catch (err) {
      error.value = String(err);
      throw err;
    } finally {
      loading.value = false;
    }
  }

  return {
    hosts,
    loading,
    error,
    loadHosts,
    saveHost,
    deleteHost,
  };
});
