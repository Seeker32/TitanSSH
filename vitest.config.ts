import { defineConfig } from 'vitest/config';
import vue from '@vitejs/plugin-vue';
import path from 'path';

export default defineConfig({
  plugins: [vue()],
  resolve: {
    alias: {
      '@': path.resolve(__dirname, './src'),
      '@tauri-apps/api/tauri': path.resolve(__dirname, './src/test/mocks/tauri.ts'),
      '@tauri-apps/api/event': path.resolve(__dirname, './src/test/mocks/event.ts'),
    },
  },
  test: {
    globals: true,
    environment: 'jsdom',
  },
});
