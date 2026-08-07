import { useEffect, useRef } from 'react';
import type { MouseEvent as ReactMouseEvent } from 'react';
import { FitAddon } from '@xterm/addon-fit';
import { Terminal } from '@xterm/xterm';
import { listen } from '@tauri-apps/api/event';
import { useThemeStore } from '@/stores/theme';

interface Props {
  sessionId: string;
  active: boolean;
  onInput: (event: { sessionId: string; data: string }) => void;
  onResize: (event: { sessionId: string; cols: number; rows: number }) => void;
}

const lightTheme = {
  background: '#ffffff', foreground: '#0f172a', cursor: '#059669', black: '#1e293b', red: '#dc2626',
  green: '#059669', yellow: '#d97706', blue: '#2563eb', magenta: '#9333ea', cyan: '#0891b2', white: '#f1f5f9',
  brightBlack: '#334155', brightRed: '#ef4444', brightGreen: '#10b981', brightYellow: '#f59e0b',
  brightBlue: '#3b82f6', brightMagenta: '#a855f7', brightCyan: '#06b6d4', brightWhite: '#ffffff',
};

const darkTheme = {
  background: '#0b1118', foreground: '#e6eff6', cursor: '#8ed2c0', black: '#15202b', red: '#ef4444',
  green: '#10b981', yellow: '#f59e0b', blue: '#3b82f6', magenta: '#a855f7', cyan: '#06b6d4', white: '#e2e8f0',
  brightBlack: '#334155', brightRed: '#f87171', brightGreen: '#6ee7b7', brightYellow: '#fbbf24',
  brightBlue: '#60a5fa', brightMagenta: '#c084fc', brightCyan: '#22d3ee', brightWhite: '#ffffff',
};

/** 挂载 xterm，并将输入、尺寸和后端数据流连接到指定会话。 */
export default function XtermView({ sessionId, active, onInput, onResize }: Props) {
  const containerRef = useRef<HTMLDivElement>(null);
  const thumbRef = useRef<HTMLDivElement>(null);
  const terminalRef = useRef<Terminal | null>(null);
  const fitAddonRef = useRef<FitAddon | null>(null);
  const viewportRef = useRef<HTMLElement | null>(null);
  const activeRef = useRef(active);
  const inputRef = useRef(onInput);
  const resizeRef = useRef(onResize);
  const theme = useThemeStore((state) => state.theme);
  activeRef.current = active;
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
      theme: useThemeStore.getState().theme === 'dark' ? darkTheme : lightTheme,
      allowTransparency: true,
    });
    const addon = new FitAddon();
    terminalRef.current = terminal;
    fitAddonRef.current = addon;
    terminal.loadAddon(addon);
    terminal.open(container);
    const viewport = container.querySelector<HTMLElement>('.xterm-viewport');
    viewportRef.current = viewport;
    viewport?.style.setProperty('scrollbar-width', 'none');
    viewport?.addEventListener('scroll', updateThumb, { passive: true });
    const screen = container.querySelector('.xterm-screen');
    const mutationObserver = new MutationObserver(updateThumb);
    if (screen) mutationObserver.observe(screen, { childList: true, subtree: true, attributes: true });
    const inputDisposable = terminal.onData((data) => inputRef.current({ sessionId, data }));
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
    if (terminalRef.current) terminalRef.current.options.theme = theme === 'dark' ? darkTheme : lightTheme;
  }, [theme]);

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
    style={{ background: theme === 'dark' ? darkTheme.background : lightTheme.background }}>
    <div className="custom-scrollbar"><div ref={thumbRef} className="custom-scrollbar__thumb" onMouseDown={startThumbDrag} /></div>
  </div>;
}
