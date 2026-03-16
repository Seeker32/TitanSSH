<script setup lang="ts">
import type { ServerStatus } from '@/types/monitor';

defineProps<{
  status: ServerStatus | null;
}>();
</script>

<template>
  <section class="status-panel">
    <div class="panel-title">
      <p>服务器状态</p>
      <strong>{{ status?.ip || '未连接' }}</strong>
    </div>

    <div v-if="status" class="status-grid">
      <article>
        <span>Uptime</span>
        <strong>{{ status.uptime_text }}</strong>
      </article>
      <article>
        <span>Load</span>
        <strong>{{ status.load1.toFixed(2) }} / {{ status.load5.toFixed(2) }} / {{ status.load15.toFixed(2) }}</strong>
      </article>
      <article>
        <span>CPU</span>
        <strong>{{ status.cpu_percent.toFixed(1) }}%</strong>
      </article>
      <article>
        <span>Memory</span>
        <strong>{{ status.memory_used_mb }} / {{ status.memory_total_mb }} MB</strong>
        <div class="bar"><div :style="{ width: `${status.memory_percent}%` }" /></div>
      </article>
      <article>
        <span>Swap</span>
        <strong>{{ status.swap_used_mb }} / {{ status.swap_total_mb }} MB</strong>
        <div class="bar"><div :style="{ width: `${status.swap_percent}%` }" /></div>
      </article>
      <article>
        <span>Updated</span>
        <strong>{{ new Date(status.updated_at * 1000).toLocaleTimeString() }}</strong>
      </article>
    </div>

    <div v-else class="placeholder">
      连接建立后，这里会每 2 秒刷新一次服务器状态。
    </div>
  </section>
</template>

<style scoped>
.status-panel {
  padding: 20px;
  border: 1px solid var(--color-border);
  border-radius: 24px;
  background: var(--color-card-bg);
}

.panel-title p,
article span {
  margin: 0;
  color: var(--color-text-secondary);
}

.panel-title strong {
  display: block;
  margin-top: 4px;
  font-size: 22px;
  color: var(--color-text-primary);
}

.status-grid {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: 16px;
  margin-top: 18px;
}

article {
  padding: 16px;
  border-radius: 18px;
  background: var(--color-card-bg);
}

article strong {
  display: block;
  margin-top: 6px;
  color: var(--color-text-primary);
}

.bar {
  height: 8px;
  margin-top: 12px;
  overflow: hidden;
  border-radius: 999px;
  background: var(--color-border);
}

.bar div {
  height: 100%;
  border-radius: inherit;
  background: linear-gradient(90deg, var(--color-accent), var(--color-accent-light));
}

.placeholder {
  margin-top: 18px;
  padding: 18px;
  border: 1px dashed var(--color-border);
  border-radius: 16px;
  color: var(--color-text-secondary);
}

@media (max-width: 860px) {
  .status-grid {
    grid-template-columns: 1fr;
  }
}
</style>
