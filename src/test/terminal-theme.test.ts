import { beforeEach, describe, expect, it, vi } from 'vitest';
import { terminalThemes, TERMINAL_THEME_NAMES } from '@/components/terminal/XtermView';
import { readTerminalTheme, useTerminalThemeStore } from '@/stores/terminal-theme';

/** 计算两个十六进制色值的 WCAG 对比度。 */
function contrastRatio(first: string, second: string) {
  /** 将单个 sRGB 通道转换为相对亮度分量。 */
  function channel(value: string) {
    const normalized = Number.parseInt(value, 16) / 255;
    return normalized <= 0.03928 ? normalized / 12.92 : ((normalized + 0.055) / 1.055) ** 2.4;
  }
  /** 计算色值的相对亮度。 */
  function luminance(color: string) {
    const channels = [color.slice(1, 3), color.slice(3, 5), color.slice(5, 7)].map(channel);
    return 0.2126 * channels[0] + 0.7152 * channels[1] + 0.0722 * channels[2];
  }
  const [lighter, darker] = [luminance(first), luminance(second)].sort((a, b) => b - a);
  return (lighter + 0.05) / (darker + 0.05);
}

describe('SSH terminal theme preference', () => {
  beforeEach(() => {
    localStorage.removeItem('terminal-theme');
    useTerminalThemeStore.setState(useTerminalThemeStore.getInitialState(), true);
  });

  it('defaults and falls back to light when persisted storage is invalid or unavailable', () => {
    expect(readTerminalTheme()).toBe('light');
    localStorage.setItem('terminal-theme', 'unknown');
    expect(readTerminalTheme()).toBe('light');
    vi.spyOn(Storage.prototype, 'getItem').mockImplementationOnce(() => { throw new Error('unavailable'); });
    expect(readTerminalTheme()).toBe('light');
  });

  it('persists the selected global terminal theme without changing the application theme', () => {
    useTerminalThemeStore.getState().setTerminalTheme('dracula');
    expect(useTerminalThemeStore.getState().terminalTheme).toBe('dracula');
    expect(localStorage.getItem('terminal-theme')).toBe('dracula');
  });

  it('keeps the selected theme in memory when local persistence fails', () => {
    vi.spyOn(Storage.prototype, 'setItem').mockImplementationOnce(() => { throw new Error('unavailable'); });
    useTerminalThemeStore.getState().setTerminalTheme('dracula');
    expect(useTerminalThemeStore.getState().terminalTheme).toBe('dracula');
  });

  it('provides exactly six complete built-in terminal palettes', () => {
    expect(TERMINAL_THEME_NAMES).toEqual(['light', 'dark', 'oneDark', 'dracula', 'solarizedLight', 'solarizedDark']);
    for (const theme of TERMINAL_THEME_NAMES) {
      expect(terminalThemes[theme]).toEqual(expect.objectContaining({
        background: expect.any(String), foreground: expect.any(String), cursor: expect.any(String),
        selectionBackground: expect.any(String), black: expect.any(String), red: expect.any(String),
        green: expect.any(String), yellow: expect.any(String), blue: expect.any(String), magenta: expect.any(String),
        cyan: expect.any(String), white: expect.any(String), brightBlack: expect.any(String), brightRed: expect.any(String),
        brightGreen: expect.any(String), brightYellow: expect.any(String), brightBlue: expect.any(String),
        brightMagenta: expect.any(String), brightCyan: expect.any(String), brightWhite: expect.any(String),
      }));
    }
  });

  it('keeps terminal text, cursor, selection, and ANSI colors readable against each palette background', () => {
    for (const theme of TERMINAL_THEME_NAMES) {
      const palette = terminalThemes[theme];
      expect(contrastRatio(palette.foreground, palette.background)).toBeGreaterThanOrEqual(4.5);
      expect(contrastRatio(palette.cursor, palette.background)).toBeGreaterThanOrEqual(3);
      expect(contrastRatio(palette.foreground, palette.selectionBackground)).toBeGreaterThanOrEqual(3);
      for (const [name, color] of Object.entries(palette)) {
        if (!['background', 'foreground', 'cursor', 'selectionBackground'].includes(name)) {
          expect(contrastRatio(color, palette.background), `${theme}.${name}`).toBeGreaterThanOrEqual(3);
        }
      }
    }
  });
});
