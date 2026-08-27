import { useEffect, useRef } from 'react';
import type { MouseEvent as ReactMouseEvent } from 'react';
import { FitAddon } from '@xterm/addon-fit';
import { Terminal } from '@xterm/xterm';
import { listen } from '@tauri-apps/api/event';
import { terminalThemes } from './terminalThemes';
import { useTerminalThemeStore } from '@/stores/terminal-theme';

export { terminalThemes, TERMINAL_THEME_NAMES } from './terminalThemes';

const terminalTextEncoder = new TextEncoder();

/** 将 xterm 的 binary 字符串按单字节编码还原为 PTY 输入字节。 */
function binaryStringToBytes(data: string): Uint8Array {
  const bytes = new Uint8Array(data.length);
  for (let index = 0; index < data.length; index += 1) {
    bytes[index] = data.charCodeAt(index) & 0xff;
  }
  return bytes;
}

interface Props {
  sessionId: string;
  active: boolean;
  /** 仅 Connected 后允许用户输入；连接完成前 xterm 不接受键盘输入。必填以强制调用方显式决定。 */
  interactive: boolean;
  onInput: (event: { sessionId: string; data: Uint8Array }) => void;
  onResize: (event: { sessionId: string; cols: number; rows: number }) => void;
}

/** 挂载 xterm，并将输入、尺寸和后端数据流连接到指定会话。 */
export default function XtermView({ sessionId, active, interactive, onInput, onResize }: Props) {
  const containerRef = useRef<HTMLDivElement>(null);
  const thumbRef = useRef<HTMLDivElement>(null);
  const terminalRef = useRef<Terminal | null>(null);
  const fitAddonRef = useRef<FitAddon | null>(null);
  const viewportRef = useRef<HTMLElement | null>(null);
  const activeRef = useRef(active);
  const interactiveRef = useRef(interactive);
  const inputRef = useRef(onInput);
  const resizeRef = useRef(onResize);
  const terminalTheme = useTerminalThemeStore((state) => state.terminalTheme);
  activeRef.current = active;
  interactiveRef.current = interactive;
  inputRef.current = onInput;
  resizeRef.current = onResize;

  /** 更新自定义滚动条的位置和高度。 */
  function updateThumb() {
    const viewport = viewportRef.current;
    const thumb = thumbRef.current;
    const container = containerRef.current;
    if (!viewport || !thumb || !container) return;
    if (viewport.scrollHeight <= viewport.clientHeight) {
      thumb.style.display = 'none';
      return;
    }
    const height = Math.max(30, (viewport.clientHeight / viewport.scrollHeight) * container.clientHeight);
    const top = (viewport.scrollTop / (viewport.scrollHeight - viewport.clientHeight)) * (container.clientHeight - height);
    thumb.style.display = 'block';
    thumb.style.height = `${height}px`;
    thumb.style.transform = `translateY(${top}px)`;
  }

  /** 重新适配终端尺寸并通知后端。 */
  function fit() {
    const terminal = terminalRef.current;
    const addon = fitAddonRef.current;
    if (!activeRef.current || !terminal || !addon) return;
    addon.fit();
    resizeRef.current({ sessionId, cols: terminal.cols, rows: terminal.rows });
  }

  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;
    const terminal = new Terminal({
      cursorBlink: true,
      fontFamily: '"SFMono-Regular", "JetBrains Mono", monospace',
      fontSize: 13,
      theme: terminalThemes[useTerminalThemeStore.getState().terminalTheme],
      allowTransparency: true,
    });
    const addon = new FitAddon();
    terminalRef.current = terminal;
    fitAddonRef.current = addon;
    terminal.loadAddon(addon);
    terminal.open(container);
    terminal.attachCustomKeyEventHandler((event) => {
      if (event.key !== 'Tab') return true;
      event.preventDefault();
      event.stopPropagation();
      // WebView 可能不提供 keyCode；仅在 keydown 且已连接时直接转发 Tab，避免重复输入
      if (event.type === 'keydown' && interactiveRef.current) {
        inputRef.current({ sessionId, data: event.shiftKey ? '\x1b[Z' : '\t' });
      }
      return false;
    });
    const viewport = container.querySelector<HTMLElement>('.xterm-viewport');
    viewportRef.current = viewport;
    viewport?.style.setProperty('scrollbar-width', 'none');
    viewport?.addEventListener('scroll', updateThumb, { passive: true });
    const screen = container.querySelector('.xterm-screen');
    const mutationObserver = new MutationObserver(updateThumb);
    if (screen) mutationObserver.observe(screen, { childList: true, subtree: true, attributes: true });
    /** 仅 Connected 后向 PTY 上送字节，避免连接未完成时发送输入。 */
    function emitInput(data: Uint8Array) {
      if (!interactiveRef.current) return;
      inputRef.current({ sessionId, data });
    }
    const inputDisposable = terminal.onData((data) => emitInput(terminalTextEncoder.encode(data)));
    const binaryInputDisposable = terminal.onBinary((data) => emitInput(binaryStringToBytes(data)));
    const resizeObserver = new ResizeObserver(() => { fit(); updateThumb(); });
    resizeObserver.observe(container);
    let disposed = false;
    let unlisten: (() => void) | undefined;
    listen<{ sessionId: string; data: string }>('terminal:data', (event) => {
      if (event.payload.sessionId === sessionId && terminalRef.current) {
        terminalRef.current.write(event.payload.data);
        requestAnimationFrame(updateThumb);
      }
    }).then((cleanup) => {
      if (disposed) cleanup(); else unlisten = cleanup;
    });
    requestAnimationFrame(() => { fit(); updateThumb(); });
    return () => {
      disposed = true;
      unlisten?.();
      resizeObserver.disconnect();
      mutationObserver.disconnect();
      viewport?.removeEventListener('scroll', updateThumb);
      inputDisposable.dispose();
      binaryInputDisposable.dispose();
      terminal.dispose();
      terminalRef.current = null;
      fitAddonRef.current = null;
      viewportRef.current = null;
    };
  }, [sessionId]);

  useEffect(() => {
    if (active) requestAnimationFrame(() => { fit(); updateThumb(); });
  }, [active]);

  useEffect(() => {
    if (terminalRef.current) terminalRef.current.options.theme = terminalThemes[terminalTheme];
  }, [terminalTheme]);

  /** 拖动自定义滚动条并同步 xterm viewport。 */
  function startThumbDrag(event: ReactMouseEvent) {
    const viewport = viewportRef.current;
    const container = containerRef.current;
    const thumb = thumbRef.current;
    if (!viewport || !container || !thumb) return;
    const activeViewport = viewport;
    const activeContainer = container;
    const activeThumb = thumb;
    const startY = event.clientY;
    const startScrollTop = viewport.scrollTop;
    /** 将鼠标位移映射为终端滚动距离。 */
    function move(moveEvent: MouseEvent) {
      const ratio = (activeViewport.scrollHeight - activeViewport.clientHeight)
        / (activeContainer.clientHeight - activeThumb.clientHeight);
      activeViewport.scrollTop = startScrollTop + (moveEvent.clientY - startY) * ratio;
    }
    /** 结束滚动条拖动并清理全局监听。 */
    function stop() {
      window.removeEventListener('mousemove', move);
      window.removeEventListener('mouseup', stop);
    }
    window.addEventListener('mousemove', move);
    window.addEventListener('mouseup', stop);
    event.preventDefault();
  }

  return <div ref={containerRef} className="terminal-view" hidden={!active}
    data-interactive={interactive}
    style={{ background: terminalThemes[terminalTheme].background }}>
    <div className="custom-scrollbar"><div ref={thumbRef} className="custom-scrollbar__thumb" onMouseDown={startThumbDrag} /></div>
  </div>;
}
