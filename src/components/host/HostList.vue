<script setup lang="ts">
import type { HostConfig } from '@/types/host';

defineProps<{
  hosts: HostConfig[];
  activeHostId: string | null;
  connectedHostIds: string[];
  connectingHostIds: string[];
}>();

const emit = defineEmits<{
  open: [string];
  create: [];
  edit: [HostConfig];
  remove: [string];
}>();
</script>

<template>
  <section class="host-list">
    <div class="toolbar">
      <div>
        <p>连接列表</p>
        <strong>{{ hosts.length }} 台主机</strong>
      </div>
      <button class="create" @click="emit('create')">新建</button>
    </div>

    <div v-if="hosts.length === 0" class="empty">还没有连接配置，先创建一台主机。</div>

    <button
      v-for="host in hosts"
      :key="host.id"
      class="host-card"
      :class="{ active: activeHostId === host.id }"
      @click="emit('open', host.id)"
    >
      <div class="host-main">
        <div>
          <strong>{{ host.name }}</strong>
          <p>{{ host.host }}:{{ host.port }}</p>
        </div>
        <span
          class="dot"
          :class="{
            connected: connectedHostIds.includes(host.id),
            connecting: connectingHostIds.includes(host.id),
          }"
        />
      </div>
      <div class="host-meta">
        <span>{{ host.username }}</span>
        <span>{{ host.auth_type === 'Password' ? '密码' : '私钥' }}</span>
      </div>
      <div class="host-actions" @click.stop>
        <button class="mini" @click="emit('edit', host)">编辑</button>
        <button class="mini danger" @click="emit('remove', host.id)">删除</button>
      </div>
    </button>
  </section>
</template>

<style scoped>
.host-list {
  display: flex;
  flex-direction: column;
  gap: 12px;
}

.toolbar,
.host-main,
.host-meta,
.host-actions {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
}

.toolbar p,
.host-main p {
  margin: 0;
  color: var(--color-text-secondary);
}

.toolbar strong,
.host-main strong {
  display: block;
  color: var(--color-text-primary);
}

.create,
.mini {
  padding: 10px 14px;
  border-radius: 12px;
  border: 1px solid var(--color-border);
  color: var(--color-text-primary);
  background: var(--color-card-bg);
}

.host-card {
  width: 100%;
  padding: 16px;
  border: 1px solid var(--color-border);
  border-radius: 18px;
  text-align: left;
  color: inherit;
  background: var(--color-card-bg);
}

.host-card.active {
  border-color: var(--color-accent);
  background: var(--color-accent-bg);
}

.host-meta {
  color: var(--color-text-secondary);
  font-size: 12px;
}

.dot {
  width: 11px;
  height: 11px;
  border-radius: 50%;
  background: var(--color-status-offline);
  box-shadow: 0 0 0 4px var(--color-status-offline);
  opacity: 0.3;
}

.dot.connecting {
  background: var(--color-status-connecting);
  box-shadow: 0 0 0 4px var(--color-status-connecting);
  opacity: 0.3;
}

.dot.connected {
  background: var(--color-status-connected);
  box-shadow: 0 0 0 4px var(--color-status-connected);
  opacity: 0.3;
}

.danger {
  color: var(--color-danger);
}

.empty {
  padding: 18px;
  border: 1px dashed var(--color-border);
  border-radius: 16px;
  color: var(--color-text-secondary);
}
</style>
