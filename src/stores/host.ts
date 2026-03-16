import { defineStore } from 'pinia';
import { invoke } from '@tauri-apps/api/core';
import { ref } from 'vue';
import type { HostConfig } from '@/types/host';

export const useHostStore = defineStore('host', () => {
  const hosts = ref<HostConfig[]>([]);
  const loading = ref(false);
  const error = ref<string | null>(null);

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

  async function saveHost(host: HostConfig) {
    loading.value = true;
    error.value = null;
    try {
      hosts.value = await invoke<HostConfig[]>('save_host', { hostConfig: host });
    } catch (err) {
      error.value = String(err);
      throw err;
    } finally {
      loading.value = false;
    }
  }

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
