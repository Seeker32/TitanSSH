<script setup lang="ts">
import type { HostConfig } from '@/types/host';

interface Props {
  hosts: HostConfig[];
}

interface Emits {
  open: [hostId: string];
  create: [];
}

defineProps<Props>();
const emit = defineEmits<Emits>();

function handleOpen(hostId: string) {
  emit('open', hostId);
}

function handleCreate() {
  emit('create');
}
</script>

<template>
  <div class="home-quick-actions">
    <div class="host-section">
      <div v-if="hosts.length === 0" class="empty-state">
        <p>暂无保存的主机</p>
        <span class="hint">点击下方"新建主机"按钮添加您的第一个 SSH 连接</span>
      </div>

      <div v-else class="host-list">
        <button
          v-for="host in hosts"
          :key="host.id"
          class="host-btn"
          @click="handleOpen(host.id)"
        >
          <span class="host-name">{{ host.name || host.host }}</span>
          <span class="host-info">{{ host.username }}@{{ host.host }}:{{ host.port }}</span>
        </button>
      </div>
    </div>

    <div class="create-section">
      <button class="create-btn" @click="handleCreate">
        <span class="icon">+</span>
        <span>新建主机</span>
      </button>
    </div>
  </div>
</template>

<style scoped>
.home-quick-actions {
  display: flex;
  flex-direction: column;
  height: 100%;
  padding: 20px;
  box-sizing: border-box;
}

.host-section {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
}

.empty-state {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 8px;
  height: 100%;
  padding: 40px;
  border: 1px dashed var(--color-border);
  border-radius: 16px;
  color: var(--color-text-secondary);
}

.empty-state .hint {
  font-size: 14px;
  color: var(--color-text-tertiary);
}

.host-list {
  display: flex;
  flex-direction: column;
  gap: 10px;
}

.host-btn {
  display: flex;
  flex-direction: column;
  align-items: flex-start;
  gap: 4px;
  padding: 14px 18px;
  border: 1px solid var(--color-border);
  border-radius: 12px;
  background: var(--color-card-bg);
  cursor: pointer;
  transition: all 0.2s ease;
  text-align: left;
}

.host-btn:hover {
  border-color: var(--color-border-focus);
  background: var(--color-card-bg-hover);
  transform: translateY(-1px);
}

.host-name {
  font-size: 15px;
  font-weight: 500;
  color: var(--color-text-primary);
}

.host-info {
  font-size: 13px;
  color: var(--color-text-secondary);
  font-family: monospace;
}

.create-section {
  margin-top: 16px;
  padding-top: 16px;
  border-top: 1px solid var(--color-border);
  flex-shrink: 0;
}

.create-btn {
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 8px;
  width: 100%;
  padding: 14px 20px;
  border: 1px solid var(--color-border);
  border-radius: 12px;
  background: var(--color-card-bg);
  color: var(--color-text-primary);
  font-size: 15px;
  font-weight: 500;
  cursor: pointer;
  transition: all 0.2s ease;
}

.create-btn:hover {
  border-color: var(--color-accent);
  background: var(--color-accent-bg);
  color: var(--color-accent);
}

.create-btn .icon {
  font-size: 20px;
  font-weight: 300;
}
</style>
