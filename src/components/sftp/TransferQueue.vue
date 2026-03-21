<script setup lang="ts">
import type { TransferTask, SftpTaskStatus } from '@/types/sftp';

defineProps<{
  tasks: Map<string, TransferTask>;
}>();

const emit = defineEmits<{
  cancel: [taskId: string];
  retry: [task: TransferTask];
}>();

/** 计算传输进度百分比（0-100） */
function progressPct(task: TransferTask): number {
  if (task.total_bytes === 0) return 0;
  return Math.round((task.transferred_bytes / task.total_bytes) * 100);
}

/** 格式化速度为可读字符串 */
function formatSpeed(bps: number): string {
  if (bps === 0) return '—';
  if (bps < 1024) return `${bps} B/s`;
  if (bps < 1024 * 1024) return `${(bps / 1024).toFixed(1)} KB/s`;
  return `${(bps / 1024 / 1024).toFixed(1)} MB/s`;
}

/** 将状态枚举转换为中文标签 */
function statusLabel(status: SftpTaskStatus): string {
  const map: Record<SftpTaskStatus, string> = {
    Pending: '等待中', Running: '传输中', Done: '完成',
    Failed: '失败', Cancelled: '已取消',
  };
  return map[status] ?? status;
}

/** 判断任务是否处于活跃状态（可取消） */
function isActive(status: SftpTaskStatus): boolean {
  return status === 'Pending' || status === 'Running';
}
</script>

<template>
  <div class="transfer-queue">
    <div v-if="tasks.size === 0" class="empty-msg">暂无传输任务</div>
    <div
      v-for="task in tasks.values()"
      :key="task.task_id"
      class="task-item"
    >
      <div class="task-top">
        <span class="task-icon">{{ task.transfer_type === 'Download' ? '⬇' : '⬆' }}</span>
        <span class="task-name" :title="task.file_name">{{ task.file_name }}</span>
        <span class="task-status" :class="`task-status--${task.status.toLowerCase()}`">
          {{ statusLabel(task.status) }}
        </span>
        <button
          v-if="isActive(task.status)"
          data-testid="cancel-btn"
          class="task-btn"
          title="取消"
          @click="emit('cancel', task.task_id)"
        >✕</button>
        <button
          v-if="task.status === 'Failed' || task.status === 'Cancelled'"
          data-testid="retry-btn"
          class="task-btn"
          title="重新发起"
          @click="emit('retry', task)"
        >↺</button>
      </div>

      <div class="progress-bar" data-testid="progress-bar">
        <div
          class="progress-fill"
          data-testid="progress-fill"
          :class="{ 'progress-fill--done': task.status === 'Done' }"
          :style="{ width: `${progressPct(task)}%` }"
        />
      </div>

      <div class="task-meta">
        <span class="task-speed">{{ formatSpeed(task.speed_bps) }}</span>
        <span class="task-pct">{{ progressPct(task) }}%</span>
      </div>

      <div v-if="task.status === 'Failed' && task.error_message" class="task-error">
        {{ task.error_message }}
      </div>
    </div>
  </div>
</template>

<style scoped>
.transfer-queue { flex: 1; overflow-y: auto; padding: 4px; }
.empty-msg { padding: 16px; color: #555; font-size: 11px; text-align: center; }
.task-item {
  padding: 6px 8px; border-radius: 4px; margin-bottom: 3px;
  background: var(--color-panel-bg, #1e1e1e); border: 1px solid var(--color-border, #2a2a2a);
}
.task-top { display: flex; align-items: center; gap: 6px; margin-bottom: 4px; }
.task-icon { font-size: 10px; flex-shrink: 0; }
.task-name { flex: 1; color: #bbb; font-size: 10px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.task-status { font-size: 9px; padding: 1px 5px; border-radius: 2px; flex-shrink: 0; }
.task-status--running { background: #1a3a5a; color: #7ab8f5; }
.task-status--done { background: #1a3a1a; color: #6abf6a; }
.task-status--pending { background: #2a2a1a; color: #aaa870; }
.task-status--failed { background: #3a1a1a; color: #bf6a6a; }
.task-status--cancelled { background: #2a2a2a; color: #888; }
.task-btn { background: none; border: none; color: #555; cursor: pointer; font-size: 10px; padding: 0 2px; }
.task-btn:hover { color: #aaa; }
.progress-bar { height: 3px; background: #2a2a2a; border-radius: 2px; overflow: hidden; }
.progress-fill { height: 100%; border-radius: 2px; background: #4a9eff; transition: width 0.3s; }
.progress-fill--done { background: #4caf50; }
.task-meta { display: flex; justify-content: space-between; margin-top: 3px; }
.task-speed { color: #555; font-size: 9px; }
.task-pct { color: #666; font-size: 9px; }
.task-error { color: #bf6a6a; font-size: 9px; margin-top: 2px; }
</style>
