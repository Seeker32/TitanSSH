import { act, render } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { emitMockEvent, resetMockEvents } from '@tauri-apps/api/event';
import XtermView, { terminalThemes, TERMINAL_THEME_NAMES } from '@/components/terminal/XtermView';
import { useTerminalThemeStore } from '@/stores/terminal-theme';
import { useThemeStore } from '@/stores/theme';

const terminal = {
  cols: 80,
  rows: 24,
  options: { theme: {} },
  loadAddon: vi.fn(),
  open: vi.fn(),
  onData: vi.fn(),
  write: vi.fn(),
  dispose: vi.fn(),
};
const inputDisposable = { dispose: vi.fn() };
const fit = vi.fn();
let inputHandler: (data: string) => void;

vi.mock('@xterm/xterm', () => ({ Terminal: vi.fn(() => terminal) }));
vi.mock('@xterm/addon-fit', () => ({ FitAddon: vi.fn(() => ({ fit })) }));

describe('XtermView', () => {
  beforeEach(() => {
    resetMockEvents();
    Object.values(terminal).forEach((value) => typeof value === 'function' && value.mockClear());
    fit.mockClear();
    inputDisposable.dispose.mockClear();
    terminal.onData.mockImplementation((handler) => { inputHandler = handler; return inputDisposable; });
    vi.stubGlobal('requestAnimationFrame', (callback: FrameRequestCallback) => { callback(0); return 1; });
    useTerminalThemeStore.setState(useTerminalThemeStore.getInitialState(), true);
  });

  it('挂载时初始化终端与尺寸插件', () => {
    render(<XtermView sessionId="session-1" active onInput={vi.fn()} onResize={vi.fn()} />);
    expect(terminal.loadAddon).toHaveBeenCalledOnce();
    expect(terminal.open).toHaveBeenCalledOnce();
  });

  it('仅写入匹配 sessionId 的后端数据', async () => {
    render(<XtermView sessionId="session-1" active onInput={vi.fn()} onResize={vi.fn()} />);
    await act(async () => {});
    act(() => {
      emitMockEvent('terminal:data', { sessionId: 'session-2', data: 'skip' });
      emitMockEvent('terminal:data', { sessionId: 'session-1', data: 'hello' });
    });
    expect(terminal.write).toHaveBeenCalledTimes(1);
    expect(terminal.write).toHaveBeenCalledWith('hello');
  });

  it('用户输入携带正确会话 ID 上送', () => {
    const onInput = vi.fn();
    render(<XtermView sessionId="session-1" active onInput={onInput} onResize={vi.fn()} />);
    inputHandler('ls\r');
    expect(onInput).toHaveBeenCalledWith({ sessionId: 'session-1', data: 'ls\r' });
  });

  it('终端色板与 slate 视觉体系一致', () => {
    expect(terminalThemes.dark.background).toBe('#0f172a');
    expect(terminalThemes.light.background).toBe('#ffffff');
    expect(terminalThemes.dark.cursor).toBe('#10b981');
    expect(terminalThemes.light.cursor).toBe('#059669');
  });

  it('每个全局 SSH 终端主题都会应用到已挂载和新建终端，且不受应用主题切换影响', () => {
    const first = render(<XtermView sessionId="session-1" active onInput={vi.fn()} onResize={vi.fn()} />);
    for (const theme of TERMINAL_THEME_NAMES) {
      act(() => useTerminalThemeStore.getState().setTerminalTheme(theme));
      expect(terminal.options.theme).toEqual(terminalThemes[theme]);
    }
    act(() => useThemeStore.getState().setTheme('dark'));
    expect(terminal.options.theme).toEqual(terminalThemes.solarizedDark);
    first.unmount();
    render(<XtermView sessionId="session-2" active onInput={vi.fn()} onResize={vi.fn()} />);
    expect(terminal.options.theme).toEqual(terminalThemes.solarizedDark);
  });

  it('卸载时释放终端资源', async () => {
    const view = render(<XtermView sessionId="session-1" active onInput={vi.fn()} onResize={vi.fn()} />);
    await act(async () => {});
    view.unmount();
    expect(inputDisposable.dispose).toHaveBeenCalledOnce();
    expect(terminal.dispose).toHaveBeenCalledOnce();
  });
});
