<script setup lang="ts">
import type { SessionInfo } from '@/types/session';
import type { HostConfig } from '@/types/host';
import XtermView from './XtermView.vue';
import HomeQuickActions from '@/components/home/HomeQuickActions.vue';

defineProps<{
  sessions: SessionInfo[];
  /** 当前激活视图 ID：'home' 表示首页，其他值为 session_id */
  activeView: 'home' | string;
  hosts: HostConfig[];
}>();

const emit = defineEmits<{
  activate: ['home' | string];
  close: [string];
  input: [{ sessionId: string; data: string }];
  resize: [{ sessionId: string; cols: number; rows: number }];
  openHost: [string];
  createHost: [];
}>();
</script>

<template>
  <section class="terminal-pane">
    <div class="viewport">
      <!-- 首页视图：固定前端视图，不依赖 SessionInfo -->
      <div v-show="activeView === 'home'" class="home-view">
        <HomeQuickActions
          :hosts="hosts"
          @open="emit('openHost', $event)"
          @create="emit('createHost')"
        />
      </div>

      <!-- 真实 SSH 会话终端，XtermView 直接监听 terminal:data 事件流 -->
      <XtermView
        v-for="session in sessions"
        :key="session.session_id"
        :session-id="session.session_id"
        :active="activeView === session.session_id"
        @input="emit('input', $event)"
        @resize="emit('resize', $event)"
      />
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
  overflow: hidden;
}

.home-view {
  height: 100%;
  overflow: auto;
}
</style>
