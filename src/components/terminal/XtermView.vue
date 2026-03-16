<script setup lang="ts">
import { FitAddon } from '@xterm/addon-fit';
import { Terminal } from '@xterm/xterm';
import { nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue';
import { useThemeStore } from '@/stores/theme';

const props = defineProps<{
  sessionId: string;
  output: string;
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
let renderedLength = 0;

// 浅色主题配色
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

// 深色主题配色
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

function syncOutput(nextOutput: string) {
  if (!terminal) {
    return;
  }

  if (nextOutput.length < renderedLength) {
    terminal.clear();
    terminal.write(nextOutput);
    renderedLength = nextOutput.length;
    return;
  }

  const delta = nextOutput.slice(renderedLength);
  if (delta) {
    terminal.write(delta);
    renderedLength = nextOutput.length;
  }
}

function fit() {
  if (!fitAddon || !terminal || !props.active) {
    return;
  }
  fitAddon.fit();
  emit('resize', {
    sessionId: props.sessionId,
    cols: terminal.cols,
    rows: terminal.rows,
  });
}

function updateTerminalTheme() {
  if (!terminal) return;
  const theme = themeStore.theme === 'dark' ? darkTheme : lightTheme;
  terminal.options.theme = theme;
}

onMounted(async () => {
  const theme = themeStore.theme === 'dark' ? darkTheme : lightTheme;
  
  terminal = new Terminal({
    cursorBlink: true,
    fontFamily: '"SFMono-Regular", "JetBrains Mono", monospace',
    fontSize: 13,
    theme: theme,
  });
  fitAddon = new FitAddon();
  terminal.loadAddon(fitAddon);
  terminal.open(containerRef.value!);
  terminal.onData((data) => emit('input', { sessionId: props.sessionId, data }));

  resizeObserver = new ResizeObserver(() => fit());
  resizeObserver.observe(containerRef.value!);

  await nextTick();
  fit();
  syncOutput(props.output);
});

onBeforeUnmount(() => {
  resizeObserver?.disconnect();
  terminal?.dispose();
});

watch(() => props.output, syncOutput);
watch(
  () => props.active,
  async (active) => {
    if (active) {
      await nextTick();
      fit();
    }
  },
);

// 监听主题变化
watch(() => themeStore.theme, updateTerminalTheme);
</script>

<template>
  <div v-show="active" ref="containerRef" class="terminal-view" />
</template>

<style scoped>
.terminal-view {
  width: 100%;
  height: 100%;
  min-height: 440px;
  padding: 16px;
  border-radius: 22px;
  background: var(--color-terminal-bg);
}
</style>
