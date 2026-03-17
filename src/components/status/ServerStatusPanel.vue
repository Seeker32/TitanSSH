<script setup lang="ts">
import { NCard, NEmpty, NProgress, NStatistic, NGrid, NGridItem, NText } from 'naive-ui';
import { computed } from 'vue';
import type { ServerStatus } from '@/types/monitor';

const props = defineProps<{
  status: ServerStatus | null;
}>();

const memPercent = computed(() => props.status?.memory_percent ?? 0);
const swapPercent = computed(() => props.status?.swap_percent ?? 0);

function progressStatus(percent: number): 'success' | 'warning' | 'error' | 'default' {
  if (percent < 60) return 'success';
  if (percent < 85) return 'warning';
  return 'error';
}
</script>

<template>
  <NCard size="small" :bordered="false" class="status-panel">
    <template #header>
      <NText depth="3" style="font-size: 12px">服务器状态</NText>
      <NText strong style="display: block; margin-top: 2px">{{ status?.ip || '未连接' }}</NText>
    </template>

    <NEmpty v-if="!status" description="连接建立后，这里会每 2 秒刷新一次服务器状态" />

    <NGrid v-else :cols="2" :x-gap="12" :y-gap="12">
      <NGridItem>
        <NStatistic label="Uptime" :value="status.uptime_text" />
      </NGridItem>
      <NGridItem>
        <NStatistic label="CPU" :value="`${status.cpu_percent.toFixed(1)}%`" />
      </NGridItem>
      <NGridItem>
        <NStatistic label="Load" :value="`${status.load1.toFixed(2)}`" />
        <NText depth="3" style="font-size: 11px">
          {{ status.load5.toFixed(2) }} / {{ status.load15.toFixed(2) }}
        </NText>
      </NGridItem>
      <NGridItem>
        <NStatistic label="Updated" :value="new Date(status.updated_at * 1000).toLocaleTimeString()" />
      </NGridItem>
      <NGridItem :span="2">
        <NText depth="3" style="font-size: 12px">
          Memory {{ status.memory_used_mb }} / {{ status.memory_total_mb }} MB
        </NText>
        <NProgress
          type="line"
          :percentage="memPercent"
          :status="progressStatus(memPercent)"
          :show-indicator="false"
          style="margin-top: 4px"
        />
      </NGridItem>
      <NGridItem :span="2">
        <NText depth="3" style="font-size: 12px">
          Swap {{ status.swap_used_mb }} / {{ status.swap_total_mb }} MB
        </NText>
        <NProgress
          type="line"
          :percentage="swapPercent"
          :status="progressStatus(swapPercent)"
          :show-indicator="false"
          style="margin-top: 4px"
        />
      </NGridItem>
    </NGrid>
  </NCard>
</template>

<style scoped>
.status-panel {
  border: 1px solid var(--color-border) !important;
  border-radius: 16px !important;
}
</style>
