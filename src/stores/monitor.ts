import { defineStore } from 'pinia';
import { ref, computed } from 'vue';
import type { ServerStatus } from '@/types/monitor';
import { useSessionStore } from './session';

export const useMonitorStore = defineStore('monitor', () => {
  const statuses = ref(new Map<string, ServerStatus>());

  const activeStatus = computed(() => {
    const sessionStore = useSessionStore();
    if (sessionStore.activeSessionId) {
      return statuses.value.get(sessionStore.activeSessionId);
    }
    return null;
  });

  // Actions will be implemented in later tasks
  
  return {
    statuses,
    activeStatus,
  };
});
