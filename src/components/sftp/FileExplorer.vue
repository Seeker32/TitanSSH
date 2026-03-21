<script setup lang="ts">
import type { SftpSessionState, RemoteEntry } from '@/types/sftp';

const props = defineProps<{
  state: SftpSessionState;
}>();

const emit = defineEmits<{
  navigate: [path: string];
  select: [path: string];
  upload: [];
  download: [paths: string[]];
}>();

/** 将路径字符串拆分为面包屑片段数组 */
function pathSegments(path: string): { label: string; path: string }[] {
  const parts = path.split('/').filter(Boolean);
  return parts.map((label, i) => ({
    label,
    path: '/' + parts.slice(0, i + 1).join('/'),
  }));
}

/** 格式化文件大小为可读字符串 */
function formatSize(bytes: number): string {
  if (bytes === 0) return '—';
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}

/** 格式化毫秒时间戳为日期字符串 */
function formatDate(ms: number): string {
  if (!ms) return '';
  return new Date(ms).toLocaleDateString('zh-CN');
}

/** 处理行点击：文件触发 select，目录不触发 */
function handleRowClick(entry: RemoteEntry) {
  if (!entry.is_dir) {
    emit('select', entry.path);
  }
}

/** 处理行双击：目录触发 navigate，文件触发 download */
function handleRowDblClick(entry: RemoteEntry) {
  if (entry.is_dir) {
    emit('navigate', entry.path);
  } else {
    emit('download', [entry.path]);
  }
}
</script>

<template>
  <div class="file-explorer">
    <!-- 路径导航栏 -->
    <div class="path-bar">
      <span
        class="path-seg path-seg--root"
        role="button"
        tabindex="0"
        @click="emit('navigate', '/')"
        @keydown.enter="emit('navigate', '/')"
      >/</span>
      <template v-for="seg in pathSegments(state.currentPath)" :key="seg.path">
        <span class="path-sep">›</span>
        <span
          class="path-seg"
          role="button"
          tabindex="0"
          @click="emit('navigate', seg.path)"
          @keydown.enter="emit('navigate', seg.path)"
        >{{ seg.label }}</span>
      </template>
    </div>

    <!-- 加载中 -->
    <div v-if="state.loading" class="state-msg">加载中...</div>

    <!-- 错误 -->
    <div v-else-if="state.error" class="state-msg state-msg--error">{{ state.error }}</div>

    <!-- 空目录 -->
    <div v-else-if="state.entries.length === 0" class="state-msg">空目录</div>

    <!-- 文件列表 -->
    <div v-else class="file-list">
      <div
        v-for="entry in state.entries"
        :key="entry.path"
        data-testid="file-row"
        class="file-row"
        :class="{ 'file-row--selected': state.selectedPaths.has(entry.path) }"
        role="row"
        tabindex="0"
        @click="handleRowClick(entry)"
        @dblclick="handleRowDblClick(entry)"
        @keydown.enter="handleRowDblClick(entry)"
      >
        <span class="file-icon">{{ entry.is_dir ? '📁' : '📄' }}</span>
        <span class="file-name" :class="{ 'file-name--dir': entry.is_dir }">{{ entry.name }}</span>
        <span class="file-size">{{ formatSize(entry.size) }}</span>
        <span class="file-date">{{ formatDate(entry.modified_at) }}</span>
      </div>
    </div>
  </div>
</template>

<style scoped>
.file-explorer { display: flex; flex-direction: column; height: 100%; min-height: 0; }
.path-bar {
  display: flex; align-items: center; gap: 4px;
  padding: 0 10px; height: 28px; flex-shrink: 0;
  background: var(--color-panel-bg); border-bottom: 1px solid var(--color-border);
  font-size: 11px;
}
.path-seg { color: #aaa; cursor: pointer; }
.path-seg:hover { color: #e0e0e0; }
.path-sep { color: #444; }
.state-msg { padding: 16px; color: #666; font-size: 12px; text-align: center; }
.state-msg--error { color: #bf6a6a; }
.file-list { flex: 1; overflow-y: auto; padding: 4px 0; }
.file-row {
  display: flex; align-items: center; gap: 8px;
  padding: 4px 12px; cursor: pointer; border-radius: 2px; margin: 0 4px;
  font-size: 11px;
}
.file-row:hover { background: var(--color-hover, #252525); }
.file-row--selected { background: var(--color-selected, #1a3a5a); }
.file-icon { width: 14px; flex-shrink: 0; }
.file-name { flex: 1; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; color: #ccc; }
.file-name--dir { color: #7ab8f5; }
.file-size { color: #555; width: 60px; text-align: right; flex-shrink: 0; }
.file-date { color: #444; width: 80px; text-align: right; flex-shrink: 0; }
</style>
