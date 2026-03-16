import { ref, watchEffect } from 'vue';
import { defineStore } from 'pinia';

type Theme = 'light' | 'dark';

export const useThemeStore = defineStore('theme', () => {
  const theme = ref<Theme>('dark');

  function setTheme(newTheme: Theme) {
    theme.value = newTheme;
    document.documentElement.setAttribute('data-theme', newTheme);
    localStorage.setItem('theme', newTheme);
  }

  function toggleTheme() {
    setTheme(theme.value === 'dark' ? 'light' : 'dark');
  }

  function initTheme() {
    const savedTheme = localStorage.getItem('theme') as Theme | null;
    const systemPrefersDark = window.matchMedia('(prefers-color-scheme: dark)').matches;
    const initialTheme = savedTheme ?? (systemPrefersDark ? 'dark' : 'light');
    setTheme(initialTheme);
  }

  watchEffect(() => {
    document.documentElement.setAttribute('data-theme', theme.value);
  });

  return {
    theme,
    setTheme,
    toggleTheme,
    initTheme,
  };
});
