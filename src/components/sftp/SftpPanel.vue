<script setup lang="ts">
import { ref, onMounted, onBeforeUnmount } from 'vue';
import type { SftpSessionState, TransferTask } from '@/types/sftp';
import FileExplorer from './FileExplorer.vue';
import TransferQueue from './TransferQueue.vue';

const props = defineProps<{
  /** 当前激活的 session ID，用于向父组件传递操作上下文 */
  sessionId: string;
  state: SftpSessionState | null;
}>();

const emit = defineEmits<{
  navigate: [sessionId: string, path: string];
  select: [sessionId: string, path: string];
  upload: [sessionId: string, remotePath: string];
  download: [sessionId: string, paths: string[]];
  cancel: [taskId: string];
  retry: [task: TransferTask];
}>();

/** 当前激活的子视图：文件浏览器或传输队列 */
const activeTab = ref<'explorer' | 'queue'>('explorer');

/** 面板高度（px），默认 280，最小 120，最大 600 */
const panelHeight = ref(280);
const MIN_HEIGHT = 120;
const MAX_HEIGHT = 600;

/** 拖拽状态 */
let isDragging = false;
let dragStartY = 0;
let dragStartHeight = 0;

/** 钳制高度到合法范围 */
function clampHeight(h: number): number {
  return Math.max(MIN_HEIGHT, Math.min(MAX_HEIGHT, h));
}

/** 开始拖拽 resizer */
function startResize(event: PointerEvent) {
  isDragging = true;
  dragStartY = event.clientY;
  dragStartHeight = panelHeight.value;
  document.body.classList.add('sftp-resizing');
}

/** 拖拽过程中更新面板高度（向上拖拽增大高度） */
function handlePointerMove(event: PointerEvent) {
  if (!isDragging) return;
  const delta = dragStartY - event.clientY;
  panelHeight.value = clampHeight(dragStartHeight + delta);
}

/** 结束拖拽 */
function stopResize() {
  if (!isDragging) return;
  isDragging = false;
  document.body.classList.remove('sftp-resizing');
}

/** 窗口 resize 时重新校验高度边界 */
function handleWindowResize() {
  panelHeight.value = clampHeight(panelHeight.value);
}

onMounted(() => {
  window.addEventListener('pointermove', handlePointerMove);
  window.addEventListener('pointerup', stopResize);
  window.addEventListener('resize', handleWindowResize);
});

onBeforeUnmount(() => {
  window.removeEventListener('pointermove', handlePointerMove);
  window.removeEventListener('pointerup', stopResize);
  window.removeEventListener('resize', handleWindowResize);
});
</script>

<template>
  <div class="sftp-panel" :style="{ height: `${panelHeight}px` }">
    <!-- Resizer 拖拽分割线 -->
    <div
      data-testid="sftp-resizer"
      class="sftp-resizer"
      role="separator"
      aria-orientation="horizontal"
      @pointerdown="startResize"
    />

    <!-- 面板头部：tab 切换 -->
    <div class="sftp-header">
      <button
        data-testid="tab-explorer"
        class="sftp-tab"
        :class="{ 'sftp-tab--active': activeTab === 'explorer' }"
        @click="activeTab = 'explorer'"
      >文件浏览器</button>
      <button
        data-testid="tab-queue"
        class="sftp-tab"
        :class="{ 'sftp-tab--active': activeTab === 'queue' }"
        @click="activeTab = 'queue'"
      >传输队列</button>
    </div>

    <!-- 无 session 占位 -->
    <div v-if="!state" class="sftp-placeholder">请选择会话</div>

    <!-- 文件浏览器视图 -->
    <FileExplorer
      v-else-if="activeTab === 'explorer'"
      data-testid="file-explorer"
      :state="state"
      @navigate="(path) => emit('navigate', props.sessionId, path)"
      @select="(path) => emit('select', props.sessionId, path)"
      @upload="emit('upload', props.sessionId, state.currentPath)"
      @download="(paths) => emit('download', props.sessionId, paths)"
    />

    <!-- 传输队列视图 -->
    <TransferQueue
      v-else
      data-testid="transfer-queue"
      :tasks="state.tasks"
      @cancel="(taskId) => emit('cancel', taskId)"
      @retry="(task) => emit('retry', task)"
    />
  </div>
</template>

<style scoped>
.sftp-panel {
  display: flex;
  flex-direction: column;
  flex-shrink: 0;
  border-top: 1px solid var(--color-border);
  background: var(--color-panel-bg);
  overflow: hidden;
}

.sftp-resizer {
  height: 5px;
  flex-shrink: 0;
  cursor: row-resize;
  background: transparent;
  position: relative;
}

.sftp-resizer::before {
  content: '';
  position: absolute;
  left: 0;
  right: 0;
  top: 2px;
  height: 1px;
  background: var(--color-border);
  transition: background 0.2s;
}

.sftp-resizer:hover::before {
  background: var(--color-accent, #4a9eff);
}

.sftp-header {
  display: flex;
  align-items: center;
  height: 28px;
  flex-shrink: 0;
  border-bottom: 1px solid var(--color-border);
  padding: 0 8px;
  gap: 4px;
}

.sftp-tab {
  background: none;
  border: none;
  color: #666;
  font-size: 11px;
  padding: 2px 8px;
  cursor: pointer;
  border-radius: 2px;
}

.sftp-tab:hover { color: #aaa; }
.sftp-tab--active { color: #e0e0e0; background: var(--color-hover, #252525); }

.sftp-placeholder {
  flex: 1;
  display: flex;
  align-items: center;
  justify-content: center;
  color: #444;
  font-size: 12px;
}
</style>
