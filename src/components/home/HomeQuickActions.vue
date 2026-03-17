<script setup lang="ts">
import { NButton, NEmpty, NScrollbar, NText } from 'naive-ui';
import type { HostConfig } from '@/types/host';

defineProps<{
  hosts: HostConfig[];
}>();

const emit = defineEmits<{
  open: [hostId: string];
  create: [];
}>();
</script>

<template>
  <div class="home-quick-actions">
    <NScrollbar style="flex: 1; min-height: 0">
      <NEmpty
        v-if="hosts.length === 0"
        description="暂无保存的主机，点击下方按钮添加第一个 SSH 连接"
        style="margin-top: 60px"
      />
      <div v-else class="host-list">
        <div
          v-for="host in hosts"
          :key="host.id"
          class="host-btn"
          @click="emit('open', host.id)"
        >
          <NText strong>{{ host.name || host.host }}</NText>
          <NText depth="3" style="font-size: 13px; font-family: monospace">
            {{ host.username }}@{{ host.host }}:{{ host.port }}
          </NText>
        </div>
      </div>
    </NScrollbar>

    <div class="create-section">
      <NButton block @click="emit('create')">+ 新建主机</NButton>
    </div>
  </div>
</template>

<style scoped>
.home-quick-actions {
  display: flex;
  flex-direction: column;
  height: 100%;
  padding: 20px;
  gap: 16px;
}

.host-list {
  display: flex;
  flex-direction: column;
  gap: 10px;
  padding-right: 4px;
}

.host-btn {
  display: flex;
  flex-direction: column;
  gap: 4px;
  padding: 14px 18px;
  border: 1px solid var(--color-border);
  border-radius: 12px;
  background: var(--color-card-bg);
  cursor: pointer;
  transition: all 0.15s ease;
}

.host-btn:hover {
  border-color: var(--color-border-focus);
  background: var(--color-card-bg-hover);
  transform: translateY(-1px);
}

.create-section {
  flex-shrink: 0;
  padding-top: 12px;
  border-top: 1px solid var(--color-border);
}
</style>
