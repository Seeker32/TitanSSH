<script setup lang="ts">
import { NText } from 'naive-ui';

defineProps<{
  statusText: string;
  userText: string;
  hostText: string;
}>();
</script>

<template>
  <footer class="app-footer">
    <NText depth="3" style="font-size: 12px">{{ statusText }}</NText>
    <NText depth="3" style="font-size: 12px">{{ userText }}</NText>
    <NText depth="3" style="font-size: 12px">{{ hostText }}</NText>
    <NText depth="3" style="font-size: 12px">UTF-8</NText>
    <NText depth="3" style="font-size: 12px">SSH</NText>
  </footer>
</template>

<style scoped>
.app-footer {
  display: flex;
  gap: 24px;
  align-items: center;
  padding: 8px 20px;
  border-top: 1px solid var(--color-border);
  flex-shrink: 0;
}
</style>
