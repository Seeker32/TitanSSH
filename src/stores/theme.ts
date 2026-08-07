import { create } from 'zustand';

export type Theme = 'light' | 'dark';

interface ThemeState {
  theme: Theme;
  setTheme: (theme: Theme) => void;
  toggleTheme: () => void;
  initTheme: () => void;
}

export const useThemeStore = create<ThemeState>((set, get) => ({
  theme: 'dark',

  /** 设置主题，并同步到 DOM 与本地存储。 */
  setTheme(theme) {
    document.documentElement.setAttribute('data-theme', theme);
    localStorage.setItem('theme', theme);
    set({ theme });
  },

  /** 在明暗主题之间切换。 */
  toggleTheme() {
    get().setTheme(get().theme === 'dark' ? 'light' : 'dark');
  },

  /** 从本地配置或系统偏好初始化主题。 */
  initTheme() {
    const saved = localStorage.getItem('theme') as Theme | null;
    const system = window.matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light';
    get().setTheme(saved ?? system);
  },
}));
