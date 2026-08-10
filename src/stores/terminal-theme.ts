import { create } from 'zustand';
import { TERMINAL_THEME_NAMES, type TerminalTheme } from '@/components/terminal/terminalThemes';

const TERMINAL_THEME_KEY = 'terminal-theme';

/** 读取已保存的 SSH 终端主题；无效或不可用的本地存储回退为浅色。 */
export function readTerminalTheme(): TerminalTheme {
  try {
    const saved = localStorage.getItem(TERMINAL_THEME_KEY);
    return TERMINAL_THEME_NAMES.includes(saved as TerminalTheme) ? saved as TerminalTheme : 'light';
  } catch {
    return 'light';
  }
}

interface TerminalThemeState {
  terminalTheme: TerminalTheme;
  setTerminalTheme: (theme: TerminalTheme) => void;
}

/** 保存跨会话共享的 SSH 终端主题偏好。 */
export const useTerminalThemeStore = create<TerminalThemeState>((set) => ({
  terminalTheme: readTerminalTheme(),
  /** 设置全局 SSH 终端主题；存储不可用时仍立即应用至当前运行时。 */
  setTerminalTheme(terminalTheme) {
    try { localStorage.setItem(TERMINAL_THEME_KEY, terminalTheme); } catch { /* 存储不可用时保留内存偏好。 */ }
    set({ terminalTheme });
  },
}));
