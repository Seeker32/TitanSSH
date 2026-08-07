import { defineConfig } from 'vitest/config';
import react from '@vitejs/plugin-react';
import path from 'node:path';

export default defineConfig({
  plugins: [react()],
  resolve: {
    alias: {
      '@': path.resolve(__dirname, './src'),
      '@tauri-apps/api/tauri': path.resolve(__dirname, './src/test/mocks/tauri.ts'),
      '@tauri-apps/api/core': path.resolve(__dirname, './src/test/mocks/tauri.ts'),
      '@tauri-apps/api/event': path.resolve(__dirname, './src/test/mocks/event.ts'),
      '@tauri-apps/plugin-dialog': path.resolve(__dirname, './src/test/mocks/dialog.ts'),
    },
  },
  test: {
    globals: true,
    environment: 'jsdom',
    setupFiles: ['./src/test/setup.ts'],
    exclude: ['e2e/**', 'node_modules/**'],
  },
});
