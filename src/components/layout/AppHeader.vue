<script setup lang="ts">
import { useThemeStore } from '@/stores/theme';

const themeStore = useThemeStore();
</script>

<template>
  <header class="app-header">
    <div>
      <p class="eyebrow">Stage 1</p>
      <h1>SSH Terminal Manager</h1>
    </div>
    <div class="header-right">
      <button class="theme-toggle" @click="themeStore.toggleTheme()">
        <span v-if="themeStore.theme === 'dark'" class="theme-icon">🌙</span>
        <span v-else class="theme-icon">☀️</span>
      </button>
    </div>
  </header>
</template>

<style scoped>
.app-header {
  display: flex;
  align-items: flex-end;
  justify-content: space-between;
  gap: 24px;
  padding: 24px 28px 18px;
}

.header-right {
  display: flex;
  align-items: center;
  gap: 16px;
}

.eyebrow {
  margin: 0 0 4px;
  color: var(--color-accent);
  font-size: 12px;
  letter-spacing: 0.16em;
  text-transform: uppercase;
}

h1 {
  margin: 0;
  font-size: 32px;
  line-height: 1.1;
  color: var(--color-text-primary);
}

.theme-toggle {
  padding: 10px 14px;
  border: 1px solid var(--color-border);
  border-radius: 12px;
  color: var(--color-text-secondary);
  background: var(--color-card-bg);
  transition: all 0.2s ease;
}

.theme-toggle:hover {
  color: var(--color-text-primary);
  background: var(--color-card-bg-hover);
  border-color: var(--color-border-focus);
  transform: scale(1.05);
}

.theme-icon {
  font-size: 18px;
}
</style>
