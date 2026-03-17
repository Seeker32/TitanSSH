<script setup lang="ts">
import { NButton, NText } from 'naive-ui';
import { useThemeStore } from '@/stores/theme';

const themeStore = useThemeStore();
</script>

<template>
  <header class="app-header">
    <div>
      <NText style="font-size: 11px; letter-spacing: 0.16em; text-transform: uppercase; color: var(--color-accent)">
        Titan SSH
      </NText>
      <NText strong style="display: block; font-size: 20px; margin-top: 2px">
        SSH Terminal Manager
      </NText>
    </div>
    <NButton text @click="themeStore.toggleTheme()">
      {{ themeStore.theme === 'dark' ? '🌙' : '☀️' }}
    </NButton>
  </header>
</template>

<style scoped>
.app-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 16px 20px 12px;
  border-bottom: 1px solid var(--color-border);
  flex-shrink: 0;
}
</style>
