<script setup lang="ts">
import { FitAddon } from '@xterm/addon-fit';
import { Terminal } from '@xterm/xterm';
import { listen } from '@tauri-apps/api/event';
import { nextTick, onBeforeUnmount, onMounted, watch } from 'vue';
import { ref } from 'vue';
import { useThemeStore } from '@/stores/theme';

const props = defineProps<{
  sessionId: string;
  active: boolean;
}>();

const emit = defineEmits<{
  input: [{ sessionId: string; data: string }];
  resize: [{ sessionId: string; cols: number; rows: number }];
}>();

const containerRef = ref<HTMLDivElement | null>(null);
const scrollThumbRef = ref<HTMLDivElement | null>(null);
const themeStore = useThemeStore();
let terminal: Terminal | null = null;
let fitAddon: FitAddon | null = null;
let resizeObserver: ResizeObserver | null = null;
let unlistenTerminalData: (() => void) | null = null;

/** 自定义滚动条状态 */
let viewport: HTMLElement | null = null;
let isDragging = false;
let dragStartY = 0;
let dragStartScrollTop = 0;
let scrollbarObserver: MutationObserver | null = null;

/** 浅色主题配色 */
const lightTheme = {
  background: '#ffffff',
  foreground: '#0f172a',
  cursor: '#059669',
  black: '#1e293b',
  red: '#dc2626',
  green: '#059669',
  yellow: '#d97706',
  blue: '#2563eb',
  magenta: '#9333ea',
  cyan: '#0891b2',
  white: '#f1f5f9',
  brightBlack: '#334155',
  brightRed: '#ef4444',
  brightGreen: '#10b981',
  brightYellow: '#f59e0b',
  brightBlue: '#3b82f6',
  brightMagenta: '#a855f7',
  brightCyan: '#06b6d4',
  brightWhite: '#ffffff',
};

/** 深色主题配色 */
const darkTheme = {
  background: '#0b1118',
  foreground: '#e6eff6',
  cursor: '#8ed2c0',
  black: '#15202b',
  red: '#ef4444',
  green: '#10b981',
  yellow: '#f59e0b',
  blue: '#3b82f6',
  magenta: '#a855f7',
  cyan: '#06b6d4',
  white: '#e2e8f0',
  brightBlack: '#334155',
  brightRed: '#f87171',
  brightGreen: '#6ee7b7',
  brightYellow: '#fbbf24',
  brightBlue: '#60a5fa',
  brightMagenta: '#c084fc',
  brightCyan: '#22d3ee',
  brightWhite: '#ffffff',
};

/** 重新计算终端尺寸并上报后端 */
function fit() {
  if (!fitAddon || !terminal || !props.active) return;
  fitAddon.fit();
  emit('resize', {
    sessionId: props.sessionId,
    cols: terminal.cols,
    rows: terminal.rows,
  });
}

/** 根据当前主题更新终端配色 */
function updateTerminalTheme() {
  if (!terminal) return;
  terminal.options.theme = themeStore.theme === 'dark' ? darkTheme : lightTheme;
}

/**
 * 根据 viewport 的滚动状态更新自定义滚动条拇指的位置和高度。
 * 拇指高度 = 可视区域 / 内容总高度，位置按比例映射。
 */
function updateThumb() {
  if (!viewport || !scrollThumbRef.value || !containerRef.value) return;
  const { scrollTop, scrollHeight, clientHeight } = viewport;
  if (scrollHeight <= clientHeight) {
    scrollThumbRef.value.style.display = 'none';
    return;
  }
  scrollThumbRef.value.style.display = 'block';
  const trackHeight = containerRef.value.clientHeight;
  const thumbHeight = Math.max(30, (clientHeight / scrollHeight) * trackHeight);
  const maxTop = trackHeight - thumbHeight;
  const top = (scrollTop / (scrollHeight - clientHeight)) * maxTop;
  scrollThumbRef.value.style.height = `${thumbHeight}px`;
  scrollThumbRef.value.style.transform = `translateY(${top}px)`;
}

/**
 * 隐藏 xterm-viewport 原生滚动条，绑定 scroll 事件以驱动自定义滚动条。
 * @param container - xterm 挂载的容器元素
 */
function setupCustomScrollbar(container: HTMLElement) {
  viewport = container.querySelector<HTMLElement>('.xterm-viewport');
  if (!viewport) return;

  // 隐藏原生滚动条
  viewport.style.setProperty('scrollbar-width', 'none');

  // viewport 滚动时同步拇指位置
  viewport.addEventListener('scroll', updateThumb, { passive: true });

  // 监听 xterm 内容变化（行数增减）以更新拇指高度
  scrollbarObserver = new MutationObserver(updateThumb);
  const screen = container.querySelector('.xterm-screen');
  if (screen) scrollbarObserver.observe(screen, { childList: true, subtree: true, attributes: true });
}

/**
 * 鼠标按下拇指时开始拖拽，记录起始位置和 scrollTop。
 * @param e - mousedown 事件
 */
function onThumbMouseDown(e: MouseEvent) {
  if (!viewport) return;
  isDragging = true;
  dragStartY = e.clientY;
  dragStartScrollTop = viewport.scrollTop;
  e.preventDefault();
}

/**
 * 全局 mousemove 处理拖拽，将鼠标位移映射为 viewport.scrollTop 变化。
 * @param e - mousemove 事件
 */
function onMouseMove(e: MouseEvent) {
  if (!isDragging || !viewport || !containerRef.value || !scrollThumbRef.value) return;
  const { scrollHeight, clientHeight } = viewport;
  const trackHeight = containerRef.value.clientHeight;
  const thumbHeight = scrollThumbRef.value.clientHeight;
  const ratio = (scrollHeight - clientHeight) / (trackHeight - thumbHeight);
  viewport.scrollTop = dragStartScrollTop + (e.clientY - dragStartY) * ratio;
}

/** 全局 mouseup 结束拖拽 */
function onMouseUp() {
  isDragging = false;
}

onMounted(async () => {
  const theme = themeStore.theme === 'dark' ? darkTheme : lightTheme;

  terminal = new Terminal({
    cursorBlink: true,
    fontFamily: '"SFMono-Regular", "JetBrains Mono", monospace',
    fontSize: 13,
    theme,
    allowTransparency: true,
  });
  fitAddon = new FitAddon();
  terminal.loadAddon(fitAddon);
  terminal.open(containerRef.value!);

  // 初始化自定义滚动条
  setupCustomScrollbar(containerRef.value!);

  // 采集用户输入并上送后端
  terminal.onData((data) => emit('input', { sessionId: props.sessionId, data }));

  resizeObserver = new ResizeObserver(() => { fit(); updateThumb(); });
  resizeObserver.observe(containerRef.value!);

  // 全局拖拽事件
  window.addEventListener('mousemove', onMouseMove);
  window.addEventListener('mouseup', onMouseUp);

  await nextTick();
  fit();
  updateThumb();

  // 直接监听 terminal:data 事件流，按 session_id 过滤后写入 xterm 实例
  unlistenTerminalData = await listen<{ session_id: string; data: string }>(
    'terminal:data',
    (event) => {
      if (event.payload.session_id === props.sessionId && terminal) {
        terminal.write(event.payload.data);
        // 写入新数据后更新滚动条位置
        requestAnimationFrame(updateThumb);
      }
    },
  );
});

onBeforeUnmount(() => {
  resizeObserver?.disconnect();
  scrollbarObserver?.disconnect();
  viewport?.removeEventListener('scroll', updateThumb);
  window.removeEventListener('mousemove', onMouseMove);
  window.removeEventListener('mouseup', onMouseUp);
  unlistenTerminalData?.();
  terminal?.dispose();
});

watch(
  () => props.active,
  async (active) => {
    if (active) {
      await nextTick();
      fit();
      updateThumb();
    }
  },
);

// 监听主题变化并同步终端配色
watch(() => themeStore.theme, updateTerminalTheme);
</script>

<template>
  <div v-show="active" ref="containerRef" class="terminal-view">
    <!-- 自定义滚动条拇指，绝对定位叠加在终端右侧 -->
    <div class="custom-scrollbar">
      <div
        ref="scrollThumbRef"
        class="custom-scrollbar__thumb"
        @mousedown="onThumbMouseDown"
      />
    </div>
  </div>
</template>

<style scoped>
.terminal-view {
  position: relative;
  width: 100%;
  height: 100%;
  padding: 8px;
  background: v-bind('themeStore.theme === "dark" ? darkTheme.background : lightTheme.background');
}

.terminal-view :deep(.xterm) {
  height: 100%;
}

.terminal-view :deep(.xterm-viewport) {
  background-color: transparent !important;
}

.terminal-view :deep(.xterm-screen) {
  background-color: transparent !important;
}

/* 自定义滚动条轨道：绝对定位在容器右侧 */
.custom-scrollbar {
  position: absolute;
  top: 8px;
  right: 2px;
  bottom: 8px;
  width: 6px;
  pointer-events: none;
  z-index: 10;
}

/* 拇指：默认半透明，hover/拖拽时加深 */
.custom-scrollbar__thumb {
  position: absolute;
  top: 0;
  left: 0;
  width: 100%;
  display: none;
  background: rgba(148, 163, 184, 0.45);
  border-radius: 999px;
  cursor: pointer;
  pointer-events: all;
  transition: background 0.15s;
  user-select: none;
}

.custom-scrollbar__thumb:hover {
  background: rgba(148, 163, 184, 0.75);
}
</style>

<style>
/* 彻底隐藏 xterm-viewport 的原生 webkit 滚动条 */
.xterm-viewport::-webkit-scrollbar {
  width: 0 !important;
  display: none !important;
}
</style>
