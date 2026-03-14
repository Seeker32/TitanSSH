import { defineStore } from 'pinia';
import { ref } from 'vue';
import type { SessionInfo } from '@/types/session';

export const useSessionStore = defineStore('session', () => {
  const sessions = ref(new Map<string, SessionInfo>());
  const activeSessionId = ref<string | null>(null);

  // Actions will be implemented in later tasks
  
  return {
    sessions,
    activeSessionId,
  };
});
