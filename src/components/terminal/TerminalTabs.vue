<script setup lang="ts">
import { NButton } from 'naive-ui';
import type { SessionInfo } from '@/types/session';

defineProps<{
  sessions: SessionInfo[];
  /** 当前激活视图 ID：'home' 表示首页，其他值为 session_id */
  activeView: 'home' | string;
}>();

const emit = defineEmits<{
  activate: ['home' | string];
  close: [string];
}>();

/** 根据会话状态返回对应的状态圆点 CSS 类名，错误类状态（AuthFailed/Error/Timeout/Disconnected）显示红色 */
function statusDot(status: string) {
  if (status === 'Connected') return 'dot-connected';
  if (status === 'Connecting') return 'dot-connecting';
  if (status === 'AuthFailed' || status === 'Error' || status === 'Timeout' || status === 'Disconnected') return 'dot-error';
  return 'dot-offline';
}
</script>

<template>
  <div class="tab-bar">
    <!-- 固定首页标签，不可关闭 -->
    <div
      class="tab"
      :class="{ active: activeView === 'home' }"
      @click="emit('activate', 'home')"
    >
      <span v-if="activeView === 'home'" class="tab-curve tab-curve-left" />
      <span class="tab-label">首页</span>
      <span v-if="activeView === 'home'" class="tab-curve tab-curve-right" />
    </div>

    <!-- 真实 SSH 会话标签 -->
    <div
      v-for="session in sessions"
      :key="session.session_id"
      class="tab"
      :class="{ active: activeView === session.session_id }"
      @click="emit('activate', session.session_id)"
    >
      <span v-if="activeView === session.session_id" class="tab-curve tab-curve-left" />
      <span class="status-dot" :class="statusDot(session.status)" />
      <span class="tab-label">{{ session.username }}@{{ session.host }}</span>
      <NButton
        text
        size="tiny"
        class="close-btn"
        @click.stop="emit('close', session.session_id)"
      >
        ×
      </NButton>
      <span v-if="activeView === session.session_id" class="tab-curve tab-curve-right" />
    </div>
  </div>
</template>

<style scoped>
.tab-bar {
  display: flex;
  align-items: flex-end;
  height: 100%;
  padding: 0 8px;
  gap: 2px;
  overflow-x: auto;
  overflow-y: hidden;
  scrollbar-width: none;
}

.tab-bar::-webkit-scrollbar {
  display: none;
}

.tab {
  position: relative;
  display: inline-flex;
  align-items: center;
  gap: 6px;
  height: 34px;
  padding: 0 12px;
  border-radius: 8px 8px 0 0;
  color: var(--color-text-tertiary);
  background: transparent;
  cursor: pointer;
  transition: color 0.15s, background 0.15s;
  font-size: 13px;
  white-space: nowrap;
  flex-shrink: 0;
  user-select: none;
  border: 1px solid transparent;
  border-bottom: none;
}

.tab:hover {
  color: var(--color-text-secondary);
  background: var(--color-card-bg);
}

.tab.active {
  color: var(--color-text-primary);
  background: var(--color-bg-primary);
  border-color: var(--color-border);
  border-bottom-color: var(--color-bg-primary);
  margin-bottom: -5px;
  z-index: 1;
}

.tab-label {
  font-family: monospace;
  font-size: 12px;
  max-width: 160px;
  overflow: hidden;
  text-overflow: ellipsis;
}

.status-dot {
  width: 7px;
  height: 7px;
  border-radius: 50%;
  flex-shrink: 0;
}

.dot-connected  { background: var(--color-status-connected); }
.dot-connecting { background: var(--color-status-connecting); }
.dot-error      { background: var(--color-danger); }
.dot-offline    { background: var(--color-text-tertiary); }

.close-btn {
  opacity: 0;
  transition: opacity 0.15s;
  font-size: 16px !important;
  line-height: 1;
}

.tab:hover .close-btn,
.tab.active .close-btn {
  opacity: 1;
}

.tab-curve {
  position: absolute;
  bottom: 0;
  width: 8px;
  height: 8px;
  overflow: hidden;
  pointer-events: none;
}

.tab-curve::before {
  content: '';
  position: absolute;
  width: 16px;
  height: 16px;
  border-radius: 50%;
  box-shadow: 0 4px 0 4px var(--color-bg-primary);
}

.tab-curve-left {
  left: -8px;
}

.tab-curve-left::before {
  bottom: 0;
  right: 0;
  box-shadow: 4px 4px 0 4px var(--color-bg-primary);
}

.tab-curve-right {
  right: -8px;
}

.tab-curve-right::before {
  bottom: 0;
  left: 0;
  box-shadow: -4px 4px 0 4px var(--color-bg-primary);
}
</style>
