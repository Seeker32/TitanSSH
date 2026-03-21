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
const themeStore = useThemeStore();
let terminal: Terminal | null = null;
let fitAddon: FitAddon | null = null;
let resizeObserver: ResizeObserver | null = null;
let unlistenTerminalData: (() => void) | null = null;

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

onMounted(async () => {
  const theme = themeStore.theme === 'dark' ? darkTheme : lightTheme;

  terminal = new Terminal({
    cursorBlink: true,
    fontFamily: '"SFMono-Regular", "JetBrains Mono", monospace',
    fontSize: 13,
    theme,
    // 允许透明背景，确保 xterm-viewport 的 transparent 设置生效
    allowTransparency: true,
  });
  fitAddon = new FitAddon();
  terminal.loadAddon(fitAddon);
  terminal.open(containerRef.value!);

  // 采集用户输入并上送后端
  terminal.onData((data) => emit('input', { sessionId: props.sessionId, data }));

  resizeObserver = new ResizeObserver(() => fit());
  resizeObserver.observe(containerRef.value!);

  await nextTick();
  fit();

  // 直接监听 terminal:data 事件流，按 session_id 过滤后写入 xterm 实例
  unlistenTerminalData = await listen<{ session_id: string; data: string }>(
    'terminal:data',
    (event) => {
      if (event.payload.session_id === props.sessionId && terminal) {
        terminal.write(event.payload.data);
      }
    },
  );
});

onBeforeUnmount(() => {
  resizeObserver?.disconnect();
  unlistenTerminalData?.();
  terminal?.dispose();
});

watch(
  () => props.active,
  async (active) => {
    if (active) {
      await nextTick();
      fit();
    }
  },
);

// 监听主题变化并同步终端配色
watch(() => themeStore.theme, updateTerminalTheme);
</script>

<template>
  <div v-show="active" ref="containerRef" class="terminal-view" />
</template>

<style scoped>
.terminal-view {
  width: 100%;
  height: 100%;
  padding: 8px;
  /* 背景色由 xterm theme.background 控制，此处跟随深色主题默认值 */
  background: v-bind('themeStore.theme === "dark" ? darkTheme.background : lightTheme.background');
}

/* 覆盖 xterm.js 内部元素的默认白色背景 */
.terminal-view :deep(.xterm) {
  height: 100%;
}

.terminal-view :deep(.xterm-viewport) {
  background-color: transparent !important;
}

.terminal-view :deep(.xterm-screen) {
  background-color: transparent !important;
}
</style>
