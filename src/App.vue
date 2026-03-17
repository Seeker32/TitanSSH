<script setup lang="ts">
import { onMounted } from 'vue';
import { NConfigProvider, NMessageProvider, NDialogProvider } from 'naive-ui';
import HomePage from '@/pages/HomePage.vue';
import { useThemeStore } from '@/stores/theme';

const themeStore = useThemeStore();

onMounted(() => {
  themeStore.initTheme();
});
</script>

<template>
  <NConfigProvider :theme="themeStore.naiveTheme" :theme-overrides="themeStore.naiveThemeOverrides">
    <NMessageProvider>
      <NDialogProvider>
        <HomePage />
      </NDialogProvider>
    </NMessageProvider>
  </NConfigProvider>
</template>

<style>
/* CSS 变量定义 - 浅色模式（默认） */
:root {
  /* 基础颜色 */
  --color-bg-primary: #f8fafc;
  --color-bg-secondary: #e2e8f0;
  --color-bg-gradient: linear-gradient(180deg, #f8fafc 0%, #e2e8f0 100%);
  --color-glow-1: rgba(99, 102, 241, 0.1);
  --color-glow-2: rgba(16, 185, 129, 0.08);

  /* 文字颜色 */
  --color-text-primary: #0f172a;
  --color-text-secondary: #64748b;
  --color-text-tertiary: #94a3b8;
  --color-text-inverse: #f1f5f9;

  /* 强调色 */
  --color-accent: #059669;
  --color-accent-light: #10b981;
  --color-accent-bg: rgba(16, 185, 129, 0.12);

  /* 卡片/面板背景 */
  --color-card-bg: rgba(255, 255, 255, 0.6);
  --color-card-bg-hover: rgba(255, 255, 255, 0.8);
  --color-panel-bg: linear-gradient(180deg, rgba(255, 255, 255, 0.7), rgba(255, 255, 255, 0.5));

  /* 边框 */
  --color-border: rgba(148, 163, 184, 0.25);
  --color-border-light: rgba(148, 163, 184, 0.15);
  --color-border-focus: rgba(5, 150, 105, 0.4);

  /* 状态颜色 */
  --color-status-connecting: #f59e0b;
  --color-status-connected: #10b981;
  --color-status-offline: #94a3b8;
  --color-danger: #ef4444;
  --color-danger-light: #fca5a5;

  /* 终端背景 */
  --color-terminal-bg: #0b1118;
  --color-terminal-text: #e6eff6;

  /* 字体 */
  font-family: "SF Pro Text", "PingFang SC", "Helvetica Neue", sans-serif;
  font-synthesis: none;
  text-rendering: optimizeLegibility;
  -webkit-font-smoothing: antialiased;
  -moz-osx-font-smoothing: grayscale;
}

/* 深色模式 */
[data-theme="dark"] {
  /* 基础颜色 */
  --color-bg-primary: #0f172a;
  --color-bg-secondary: #020617;
  --color-bg-gradient: linear-gradient(180deg, #0f172a 0%, #020617 100%);
  --color-glow-1: rgba(56, 189, 248, 0.15);
  --color-glow-2: rgba(16, 185, 129, 0.1);

  /* 文字颜色 */
  --color-text-primary: #f1f5f9;
  --color-text-secondary: #94a3b8;
  --color-text-tertiary: #64748b;
  --color-text-inverse: #0f172a;

  /* 强调色 */
  --color-accent: #10b981;
  --color-accent-light: #6ee7b7;
  --color-accent-bg: rgba(16, 185, 129, 0.15);

  /* 卡片/面板背景 */
  --color-card-bg: rgba(255, 255, 255, 0.03);
  --color-card-bg-hover: rgba(255, 255, 255, 0.05);
  --color-panel-bg: linear-gradient(180deg, rgba(20, 30, 42, 0.92), rgba(11, 17, 24, 0.96));

  /* 边框 */
  --color-border: rgba(148, 163, 184, 0.15);
  --color-border-light: rgba(148, 163, 184, 0.1);
  --color-border-focus: rgba(16, 185, 129, 0.4);

  /* 状态颜色 */
  --color-status-connecting: #f3c467;
  --color-status-connected: #7de0ac;
  --color-status-offline: #5f7180;
  --color-danger: #f39b9b;
  --color-danger-light: #f6b2b2;
}

/* 全局背景 */
html,
body {
  margin: 0;
  min-height: 100%;
  color: var(--color-text-primary);
  background:
    radial-gradient(circle at top left, var(--color-glow-1), transparent 28%),
    radial-gradient(circle at top right, var(--color-glow-2), transparent 24%),
    var(--color-bg-gradient);
}

#app {
  margin: 0;
  min-height: 100%;
}

* {
  box-sizing: border-box;
}

button,
input,
select,
textarea {
  font: inherit;
}

button {
  cursor: pointer;
}

/* 输入框默认样式 */
input,
select,
textarea {
  width: 100%;
  padding: 12px 14px;
  border: 1px solid var(--color-border);
  border-radius: 14px;
  color: var(--color-text-primary);
  background: var(--color-card-bg);
  transition: border-color 0.2s, box-shadow 0.2s;
}

input:focus,
select:focus,
textarea:focus {
  outline: none;
  border-color: var(--color-border-focus);
  box-shadow: 0 0 0 3px var(--color-accent-bg);
}

input::placeholder,
textarea::placeholder {
  color: var(--color-text-tertiary);
}

/* 覆盖 Naive UI 输入框 focus 时四角溢出的 box-shadow */
.n-input--focus,
.n-input-wrapper:focus-within {
  box-shadow: none !important;
}

.n-base-selection--focus,
.n-base-selection:focus-within {
  box-shadow: none !important;
}

/* 按钮默认样式 */
button {
  padding: 10px 16px;
  border-radius: 12px;
  border: 1px solid var(--color-border);
  color: var(--color-text-primary);
  background: var(--color-card-bg);
  transition: all 0.2s ease;
}

button:hover {
  background: var(--color-card-bg-hover);
  border-color: var(--color-border-focus);
}

/* 主按钮样式 */
button.primary {
  color: var(--color-text-inverse);
  background: var(--color-accent);
  border-color: var(--color-accent);
}

button.primary:hover {
  background: var(--color-accent-light);
}

/* 幽灵按钮样式 */
button.ghost {
  color: var(--color-text-secondary);
  background: transparent;
}

button.ghost:hover {
  color: var(--color-text-primary);
  background: var(--color-card-bg);
}
</style>
