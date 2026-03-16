<script setup lang="ts">
defineProps<{
  statusText: string;
  userText: string;
  hostText: string;
}>();
</script>

<template>
  <footer class="app-footer">
    <span>{{ statusText }}</span>
    <span>{{ userText }}</span>
    <span>{{ hostText }}</span>
    <span>UTF-8</span>
    <span>SSH</span>
  </footer>
</template>

<style scoped>
.app-footer {
  display: grid;
  grid-template-columns: repeat(5, minmax(0, 1fr));
  gap: 12px;
  padding: 14px 24px 18px;
  border-top: 1px solid var(--color-border);
  color: var(--color-text-secondary);
  font-size: 12px;
}

@media (max-width: 860px) {
  .app-footer {
    grid-template-columns: repeat(2, minmax(0, 1fr));
  }
}
</style>
