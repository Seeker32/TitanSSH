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
  attachCustomKeyEventHandler: vi.fn(),
  onData: vi.fn(),
  write: vi.fn(),
  dispose: vi.fn(),
};
const inputDisposable = { dispose: vi.fn() };
const fit = vi.fn();
let inputHandler: (data: string) => void;
let keyHandler: ((event: KeyboardEvent) => boolean) | undefined;

vi.mock('@xterm/xterm', () => ({ Terminal: vi.fn(() => terminal) }));
vi.mock('@xterm/addon-fit', () => ({ FitAddon: vi.fn(() => ({ fit })) }));

describe('XtermView', () => {
  beforeEach(() => {
    resetMockEvents();
    Object.values(terminal).forEach((value) => typeof value === 'function' && value.mockClear());
    fit.mockClear();
    inputDisposable.dispose.mockClear();
    terminal.onData.mockImplementation((handler) => { inputHandler = handler; return inputDisposable; });
    terminal.attachCustomKeyEventHandler.mockImplementation((handler) => { keyHandler = handler; });
    keyHandler = undefined;
    vi.stubGlobal('requestAnimationFrame', (callback: FrameRequestCallback) => { callback(0); return 1; });
    useTerminalThemeStore.setState(useTerminalThemeStore.getInitialState(), true);
  });

  it('挂载时初始化终端与尺寸插件', () => {
    render(<XtermView sessionId="session-1" active interactive onInput={vi.fn()} onResize={vi.fn()} />);
    expect(terminal.loadAddon).toHaveBeenCalledOnce();
    expect(terminal.open).toHaveBeenCalledOnce();
  });

  it('仅写入匹配 sessionId 的后端数据', async () => {
    render(<XtermView sessionId="session-1" active interactive onInput={vi.fn()} onResize={vi.fn()} />);
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
    render(<XtermView sessionId="session-1" active interactive onInput={onInput} onResize={vi.fn()} />);
    inputHandler('ls\r');
    expect(onInput).toHaveBeenCalledWith({ sessionId: 'session-1', data: 'ls\r' });
  });

  it('WebView keyCode 为 0 时 Tab 仍会阻止焦点跳转并上送补全输入', () => {
    const onInput = vi.fn();
    render(<XtermView sessionId="session-1" active interactive onInput={onInput} onResize={vi.fn()} />);
    const event = {
      type: 'keydown', key: 'Tab', keyCode: 0, shiftKey: false,
      preventDefault: vi.fn(), stopPropagation: vi.fn(),
    } as unknown as KeyboardEvent;

    expect(keyHandler).toBeTypeOf('function');
    expect(keyHandler?.(event)).toBe(false);
    expect(event.preventDefault).toHaveBeenCalledOnce();
    expect(event.stopPropagation).toHaveBeenCalledOnce();
    expect(onInput).toHaveBeenCalledWith({ sessionId: 'session-1', data: '\t' });
  });

  it('Shift+Tab 仅上送一次反向补全序列', () => {
    const onInput = vi.fn();
    render(<XtermView sessionId="session-1" active interactive onInput={onInput} onResize={vi.fn()} />);
    const event = {
      type: 'keydown', key: 'Tab', keyCode: 0, shiftKey: true,
      preventDefault: vi.fn(), stopPropagation: vi.fn(),
    } as unknown as KeyboardEvent;

    keyHandler?.(event);
    expect(onInput).toHaveBeenCalledOnce();
    expect(onInput).toHaveBeenCalledWith({ sessionId: 'session-1', data: '\x1b[Z' });
  });

  it('Connected 前不接收用户输入，转为可交互后恢复上送', () => {
    const onInput = vi.fn();
    const view = render(<XtermView sessionId="session-1" active interactive={false} onInput={onInput} onResize={vi.fn()} />);
    inputHandler('ls\r');
    expect(onInput).not.toHaveBeenCalled();
    view.rerender(<XtermView sessionId="session-1" active interactive onInput={onInput} onResize={vi.fn()} />);
    inputHandler('pwd\r');
    expect(onInput).toHaveBeenCalledWith({ sessionId: 'session-1', data: 'pwd\r' });
  });

  it('终端色板与 slate 视觉体系一致', () => {
    expect(terminalThemes.dark.background).toBe('#0f172a');
    expect(terminalThemes.light.background).toBe('#ffffff');
    expect(terminalThemes.dark.cursor).toBe('#10b981');
    expect(terminalThemes.light.cursor).toBe('#059669');
  });

  it('每个全局 SSH 终端主题都会应用到已挂载和新建终端，且不受应用主题切换影响', () => {
    const first = render(<XtermView sessionId="session-1" active interactive onInput={vi.fn()} onResize={vi.fn()} />);
    for (const theme of TERMINAL_THEME_NAMES) {
      act(() => useTerminalThemeStore.getState().setTerminalTheme(theme));
      expect(terminal.options.theme).toEqual(terminalThemes[theme]);
    }
    act(() => useThemeStore.getState().setTheme('dark'));
    expect(terminal.options.theme).toEqual(terminalThemes.solarizedDark);
    first.unmount();
    render(<XtermView sessionId="session-2" active interactive onInput={vi.fn()} onResize={vi.fn()} />);
    expect(terminal.options.theme).toEqual(terminalThemes.solarizedDark);
  });

  it('卸载时释放终端资源', async () => {
    const view = render(<XtermView sessionId="session-1" active interactive onInput={vi.fn()} onResize={vi.fn()} />);
    await act(async () => {});
    view.unmount();
    expect(inputDisposable.dispose).toHaveBeenCalledOnce();
    expect(terminal.dispose).toHaveBeenCalledOnce();
  });
});
