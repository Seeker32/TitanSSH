<script setup lang="ts">
import type { SessionInfo } from '@/types/session';
import XtermView from './XtermView.vue';
import HomeQuickActions from '@/components/home/HomeQuickActions.vue';
import type { HostConfig } from '@/types/host';

defineProps<{
  sessions: SessionInfo[];
  activeSessionId: string | null;
  outputs: Map<string, string>;
  hosts: HostConfig[];
}>();

const emit = defineEmits<{
  activate: [string];
  close: [string];
  input: [{ sessionId: string; data: string }];
  resize: [{ sessionId: string; cols: number; rows: number }];
  openHost: [string];
  createHost: [];
}>();

function isHomeSession(session: SessionInfo): boolean {
  return session.isHome === true;
}
</script>

<template>
  <section class="terminal-pane">
    <div class="viewport">
      <template v-for="session in sessions" :key="session.session_id">
        <!-- Home session -->
        <div
          v-if="isHomeSession(session)"
          v-show="activeSessionId === session.session_id"
          class="home-view"
        >
          <HomeQuickActions
            :hosts="hosts"
            @open="emit('openHost', $event)"
            @create="emit('createHost')"
          />
        </div>

        <!-- Regular SSH session -->
        <XtermView
          v-else
          :session-id="session.session_id"
          :output="outputs.get(session.session_id) ?? ''"
          :active="activeSessionId === session.session_id"
          @input="emit('input', $event)"
          @resize="emit('resize', $event)"
        />
      </template>
    </div>
  </section>
</template>

<style scoped>
.terminal-pane {
  display: flex;
  flex-direction: column;
  height: 100%;
  min-height: 0;
}

.viewport {
  flex: 1;
  min-height: 0;
  border: 1px solid var(--color-border);
  border-radius: 16px;
  background: var(--color-card-bg);
  overflow: hidden;
}

.home-view {
  height: 100%;
  overflow: auto;
}

.empty {
  display: grid;
  place-items: center;
  height: 100%;
  color: var(--color-text-secondary);
}
</style>
