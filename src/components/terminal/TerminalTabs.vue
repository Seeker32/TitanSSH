<script setup lang="ts">
import type { SessionInfo } from '@/types/session';

defineProps<{
  sessions: SessionInfo[];
  activeSessionId: string | null;
}>();

const emit = defineEmits<{
  activate: [string];
  close: [string];
}>();
</script>

<template>
  <div class="tabs">
    <button
      v-for="session in sessions"
      :key="session.session_id"
      class="tab"
      :class="{ active: activeSessionId === session.session_id }"
      @click="emit('activate', session.session_id)"
    >
      <span>{{ session.isHome ? '首页' : `${session.username}@${session.host}` }}</span>
      <small v-if="!session.isHome">{{ session.status }}</small>
      <span v-if="!session.isHome" class="close" @click.stop="emit('close', session.session_id)">×</span>
    </button>
  </div>
</template>

<style scoped>
.tabs {
  display: flex;
  gap: 10px;
  overflow: auto;
  padding-bottom: 8px;
}

.tab {
  display: inline-flex;
  align-items: center;
  gap: 10px;
  min-width: 220px;
  padding: 12px 14px;
  border: 1px solid var(--color-border);
  border-radius: 16px;
  color: var(--color-text-secondary);
  background: var(--color-card-bg);
}

.tab.active {
  color: var(--color-text-primary);
  border-color: var(--color-accent);
  background: var(--color-accent-bg);
}

small {
  color: var(--color-text-tertiary);
}

.close {
  margin-left: auto;
  font-size: 20px;
  line-height: 1;
}
</style>
