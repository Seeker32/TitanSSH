import { defineStore } from 'pinia';
import { ref } from 'vue';
import type { HostConfig } from '@/types/host';

export const useHostStore = defineStore('host', () => {
  const hosts = ref<HostConfig[]>([]);
  const loading = ref(false);
  const error = ref<string | null>(null);

  // Actions will be implemented in later tasks
  
  return {
    hosts,
    loading,
    error,
  };
});
