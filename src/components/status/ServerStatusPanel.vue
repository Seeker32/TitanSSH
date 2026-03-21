<script setup lang="ts">
import { NCard, NEmpty, NProgress, NStatistic, NGrid, NGridItem, NText } from 'naive-ui';
import { computed } from 'vue';
import type { MonitorSnapshot } from '@/types/monitor';

/** 接收后端推送的监控快照，null 表示尚未连接或无数据 */
const props = defineProps<{
  snapshot: MonitorSnapshot | null;
}>();

/** CPU 使用率百分比，用于进度条渲染 */
const cpuPercent = computed(() => props.snapshot?.cpu_usage ?? 0);
/** 内存使用率百分比，用于进度条渲染 */
const memPercent = computed(() => props.snapshot?.memory_usage ?? 0);
/** 磁盘使用率百分比，用于进度条渲染 */
const diskPercent = computed(() => props.snapshot?.disk_usage ?? 0);

/** 根据使用率返回进度条颜色状态：< 60% 绿，< 85% 黄，>= 85% 红 */
function progressStatus(percent: number): 'success' | 'warning' | 'error' | 'default' {
  if (percent < 60) return 'success';
  if (percent < 85) return 'warning';
  return 'error';
}

/** 将毫秒时间戳格式化为本地时间字符串 */
const updatedAt = computed(() =>
  props.snapshot ? new Date(props.snapshot.timestamp).toLocaleTimeString() : '--'
);
</script>

<template>
  <NCard size="small" :bordered="false" class="status-panel">
    <template #header>
      <NText depth="3" style="font-size: 12px">服务器状态</NText>
      <NText strong style="display: block; margin-top: 2px">
        {{ snapshot ? '已连接' : '未连接' }}
      </NText>
    </template>

    <NEmpty v-if="!snapshot" description="连接建立后，这里会每 2 秒刷新一次服务器状态" />

    <NGrid v-else :cols="2" :x-gap="12" :y-gap="12">
      <NGridItem>
        <NStatistic label="CPU" :value="`${cpuPercent.toFixed(1)}%`" />
        <NProgress
          type="line"
          :percentage="cpuPercent"
          :status="progressStatus(cpuPercent)"
          :show-indicator="false"
          style="margin-top: 4px"
        />
      </NGridItem>
      <NGridItem>
        <NStatistic label="Memory" :value="`${memPercent.toFixed(1)}%`" />
        <NProgress
          type="line"
          :percentage="memPercent"
          :status="progressStatus(memPercent)"
          :show-indicator="false"
          style="margin-top: 4px"
        />
      </NGridItem>
      <NGridItem :span="2">
        <NStatistic label="Disk" :value="`${diskPercent.toFixed(1)}%`" />
        <NProgress
          type="line"
          :percentage="diskPercent"
          :status="progressStatus(diskPercent)"
          :show-indicator="false"
          style="margin-top: 4px"
        />
      </NGridItem>
      <NGridItem :span="2">
        <NText depth="3" style="font-size: 11px">Updated: {{ updatedAt }}</NText>
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
