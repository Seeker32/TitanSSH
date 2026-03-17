import { ref, computed, watchEffect } from 'vue';
import { defineStore } from 'pinia';
import { darkTheme, lightTheme } from 'naive-ui';
import type { GlobalThemeOverrides } from 'naive-ui';

type Theme = 'light' | 'dark';

const inputOverrides = {
  boxShadowFocus: 'none',
  borderFocus: '1px solid rgba(16, 185, 129, 0.6)',
  borderHover: '1px solid rgba(16, 185, 129, 0.4)',
};

const darkOverrides: GlobalThemeOverrides = {
  common: {
    primaryColor: '#10b981',
    primaryColorHover: '#6ee7b7',
    primaryColorPressed: '#059669',
    primaryColorSuppl: '#10b981',
    borderRadius: '12px',
    fontFamily: '"SF Pro Text", "PingFang SC", "Helvetica Neue", sans-serif',
  },
  Input: inputOverrides,
  Select: { boxShadowFocus: 'none' },
  InputNumber: { boxShadowFocus: 'none' },
};

const lightOverrides: GlobalThemeOverrides = {
  common: {
    primaryColor: '#059669',
    primaryColorHover: '#10b981',
    primaryColorPressed: '#047857',
    primaryColorSuppl: '#059669',
    borderRadius: '12px',
    fontFamily: '"SF Pro Text", "PingFang SC", "Helvetica Neue", sans-serif',
  },
  Input: inputOverrides,
  Select: { boxShadowFocus: 'none' },
  InputNumber: { boxShadowFocus: 'none' },
};

export const useThemeStore = defineStore('theme', () => {
  const theme = ref<Theme>('dark');

  const naiveTheme = computed(() => (theme.value === 'dark' ? darkTheme : lightTheme));
  const naiveThemeOverrides = computed(() =>
    theme.value === 'dark' ? darkOverrides : lightOverrides,
  );

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
    naiveTheme,
    naiveThemeOverrides,
    setTheme,
    toggleTheme,
    initTheme,
  };
});
