import { defineStore } from 'pinia';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { ref, computed } from 'vue';
import type { ServerStatus } from '@/types/monitor';
import { useSessionStore } from './session';

export const useMonitorStore = defineStore('monitor', () => {
  const statuses = ref(new Map<string, ServerStatus>());

  const activeStatus = computed(() => {
    const sessionStore = useSessionStore();
    if (sessionStore.activeSessionId) {
      return statuses.value.get(sessionStore.activeSessionId) ?? null;
    }
    return null;
  });

  function applyStatus(status: ServerStatus) {
    statuses.value = new Map(statuses.value).set(status.session_id, status);
  }

  async function fetchStatus(sessionId: string) {
    const status = await invoke<ServerStatus>('get_server_status', { sessionId });
    applyStatus(status);
    return status;
  }

  async function initListeners() {
    return listen<ServerStatus>('monitor:update', (event) => {
      applyStatus(event.payload);
    });
  }

  return {
    statuses,
    activeStatus,
    applyStatus,
    fetchStatus,
    initListeners,
  };
});
