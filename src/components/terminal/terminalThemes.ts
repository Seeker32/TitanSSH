/** SSH 终端内置主题标识；偏好全局共享，不隶属主机或会话。 */
export const TERMINAL_THEME_NAMES = ['light', 'dark', 'oneDark', 'dracula', 'solarizedLight', 'solarizedDark'] as const;

export type TerminalTheme = typeof TERMINAL_THEME_NAMES[number];

/** SSH 终端主题的展示名称。 */
export const terminalThemeLabels: Record<TerminalTheme, string> = {
  light: '浅色', dark: '深色', oneDark: 'One Dark', dracula: 'Dracula', solarizedLight: 'Solarized Light', solarizedDark: 'Solarized Dark',
};

/** 六套完整 xterm 色板，包含选区与标准、明亮 ANSI 色。 */
export const terminalThemes = {
  light: { background: '#ffffff', foreground: '#0f172a', cursor: '#059669', selectionBackground: '#bbf7d0', black: '#1e293b', red: '#dc2626', green: '#059669', yellow: '#d97706', blue: '#2563eb', magenta: '#9333ea', cyan: '#0891b2', white: '#475569', brightBlack: '#334155', brightRed: '#ef4444', brightGreen: '#059669', brightYellow: '#d97706', brightBlue: '#3b82f6', brightMagenta: '#a855f7', brightCyan: '#0891b2', brightWhite: '#0f172a' },
  dark: { background: '#0f172a', foreground: '#e2e8f0', cursor: '#10b981', selectionBackground: '#334155', black: '#94a3b8', red: '#ef4444', green: '#10b981', yellow: '#f59e0b', blue: '#3b82f6', magenta: '#a855f7', cyan: '#06b6d4', white: '#e2e8f0', brightBlack: '#cbd5e1', brightRed: '#f87171', brightGreen: '#6ee7b7', brightYellow: '#fbbf24', brightBlue: '#60a5fa', brightMagenta: '#c084fc', brightCyan: '#22d3ee', brightWhite: '#ffffff' },
  oneDark: { background: '#282c34', foreground: '#abb2bf', cursor: '#528bff', selectionBackground: '#3e4451', black: '#abb2bf', red: '#e06c75', green: '#98c379', yellow: '#d19a66', blue: '#61afef', magenta: '#c678dd', cyan: '#56b6c2', white: '#abb2bf', brightBlack: '#e6e6e6', brightRed: '#e06c75', brightGreen: '#98c379', brightYellow: '#d19a66', brightBlue: '#61afef', brightMagenta: '#c678dd', brightCyan: '#56b6c2', brightWhite: '#ffffff' },
  dracula: { background: '#282a36', foreground: '#f8f8f2', cursor: '#f8f8f0', selectionBackground: '#44475a', black: '#bd93f9', red: '#ff5555', green: '#50fa7b', yellow: '#f1fa8c', blue: '#bd93f9', magenta: '#ff79c6', cyan: '#8be9fd', white: '#f8f8f2', brightBlack: '#d6acff', brightRed: '#ff6e6e', brightGreen: '#69ff94', brightYellow: '#ffffa5', brightBlue: '#d6acff', brightMagenta: '#ff92df', brightCyan: '#a4ffff', brightWhite: '#ffffff' },
  solarizedLight: { background: '#fdf6e3', foreground: '#586e75', cursor: '#268bd2', selectionBackground: '#eee8d5', black: '#073642', red: '#dc322f', green: '#657b00', yellow: '#8a6d00', blue: '#268bd2', magenta: '#b2256d', cyan: '#007c76', white: '#586e75', brightBlack: '#002b36', brightRed: '#cb4b16', brightGreen: '#586e75', brightYellow: '#657b83', brightBlue: '#586e75', brightMagenta: '#6c71c4', brightCyan: '#586e75', brightWhite: '#002b36' },
  solarizedDark: { background: '#002b36', foreground: '#93a1a1', cursor: '#268bd2', selectionBackground: '#073642', black: '#839496', red: '#dc322f', green: '#859900', yellow: '#b58900', blue: '#268bd2', magenta: '#d33682', cyan: '#2aa198', white: '#eee8d5', brightBlack: '#eee8d5', brightRed: '#cb4b16', brightGreen: '#839496', brightYellow: '#839496', brightBlue: '#839496', brightMagenta: '#6c71c4', brightCyan: '#93a1a1', brightWhite: '#fdf6e3' },
} as const;
