<script setup lang="ts">
import { NButton, NEmpty, NScrollbar, NTag, NText, NTooltip } from 'naive-ui';
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

function statusType(hostId: string, connectedIds: string[], connectingIds: string[]) {
  if (connectedIds.includes(hostId)) return 'success';
  if (connectingIds.includes(hostId)) return 'warning';
  return 'default';
}

function statusLabel(hostId: string, connectedIds: string[], connectingIds: string[]) {
  if (connectedIds.includes(hostId)) return '已连接';
  if (connectingIds.includes(hostId)) return '连接中';
  return '离线';
}
</script>

<template>
  <section class="host-list">
    <div class="toolbar">
      <div>
        <NText depth="3" style="font-size: 12px">连接列表</NText>
        <NText strong style="display: block; margin-top: 2px">{{ hosts.length }} 台主机</NText>
      </div>
      <NButton size="small" type="primary" @click="emit('create')">新建</NButton>
    </div>

    <NScrollbar style="flex: 1; min-height: 0">
      <NEmpty v-if="hosts.length === 0" description="还没有连接配置，先创建一台主机" style="margin-top: 40px" />

      <div v-else class="host-cards">
        <div
          v-for="host in hosts"
          :key="host.id"
          class="host-card"
          :class="{ active: activeHostId === host.id }"
          @click="emit('open', host.id)"
        >
          <div class="host-main">
            <div class="host-info">
              <NText strong>{{ host.name }}</NText>
              <NText depth="3" style="font-size: 12px; font-family: monospace">
                {{ host.host }}:{{ host.port }}
              </NText>
            </div>
            <NTag
              :type="statusType(host.id, connectedHostIds, connectingHostIds)"
              size="small"
              round
            >
              {{ statusLabel(host.id, connectedHostIds, connectingHostIds) }}
            </NTag>
          </div>

          <div class="host-meta">
            <NText depth="3" style="font-size: 12px">{{ host.username }}</NText>
            <NText depth="3" style="font-size: 12px">
              {{ host.auth_type === 'Password' ? '密码' : '私钥' }}
            </NText>
          </div>

          <div class="host-actions" @click.stop>
            <NTooltip trigger="hover">
              <template #trigger>
                <NButton size="tiny" @click="emit('edit', host)">编辑</NButton>
              </template>
              编辑连接配置
            </NTooltip>
            <NTooltip trigger="hover">
              <template #trigger>
                <NButton size="tiny" type="error" @click="emit('remove', host.id)">删除</NButton>
              </template>
              删除此连接
            </NTooltip>
          </div>
        </div>
      </div>
    </NScrollbar>
  </section>
</template>

<style scoped>
.host-list {
  display: flex;
  flex-direction: column;
  gap: 12px;
  height: 100%;
  min-height: 0;
}

.toolbar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  flex-shrink: 0;
}

.host-cards {
  display: flex;
  flex-direction: column;
  gap: 10px;
  padding-right: 4px;
}

.host-card {
  padding: 14px 16px;
  border: 1px solid var(--color-border);
  border-radius: 14px;
  background: var(--color-card-bg);
  cursor: pointer;
  transition: all 0.15s ease;
}

.host-card:hover {
  border-color: var(--color-border-focus);
  background: var(--color-card-bg-hover);
}

.host-card.active {
  border-color: var(--color-accent);
  background: var(--color-accent-bg);
}

.host-main {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: 8px;
}

.host-info {
  display: flex;
  flex-direction: column;
  gap: 2px;
  min-width: 0;
}

.host-meta {
  display: flex;
  justify-content: space-between;
  margin-top: 8px;
}

.host-actions {
  display: flex;
  gap: 6px;
  margin-top: 10px;
}
</style>
