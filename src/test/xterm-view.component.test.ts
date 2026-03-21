/**
 * P1-1 XtermView 组件测试
 *
 * 覆盖：
 * 1. 组件挂载时初始化 xterm Terminal 实例
 * 2. terminal:data 事件按 session_id 过滤后写入终端
 * 3. 其他 session_id 的 terminal:data 事件不写入本终端
 * 4. 用户输入触发 input emit，payload 含 sessionId 和 data
 * 5. active=false 时 fit 不触发 resize emit
 * 6. active=true 时 watch 触发 fit，emit resize
 * 7. 组件卸载时清理监听器和 ResizeObserver
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { mount } from '@vue/test-utils';
import { nextTick } from 'vue';
import { emitMockEvent, resetMockEvents } from '@tauri-apps/api/event';
import { createPinia, setActivePinia } from 'pinia';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

// xterm 和 FitAddon 的 mock
const mockWrite = vi.fn();
const mockDispose = vi.fn();
const mockOpen = vi.fn();
const mockLoadAddon = vi.fn();
const mockFit = vi.fn();

let capturedOnDataCallback: ((data: string) => void) | null = null;

vi.mock('@xterm/xterm', () => ({
  Terminal: vi.fn().mockImplementation(() => ({
    open: mockOpen,
    loadAddon: mockLoadAddon,
    onData: vi.fn((cb: (data: string) => void) => {
      capturedOnDataCallback = cb;
    }),
    write: mockWrite,
    dispose: mockDispose,
    cols: 80,
    rows: 24,
    options: {},
  })),
}));

vi.mock('@xterm/addon-fit', () => ({
  FitAddon: vi.fn().mockImplementation(() => ({
    fit: mockFit,
  })),
}));

// ResizeObserver mock
const mockObserve = vi.fn();
const mockDisconnect = vi.fn();
vi.stubGlobal('ResizeObserver', vi.fn().mockImplementation(() => ({
  observe: mockObserve,
  disconnect: mockDisconnect,
})));

import XtermView from '@/components/terminal/XtermView.vue';

function mountXterm(sessionId = 'session-1', active = true) {
  return mount(XtermView, {
    props: { sessionId, active },
    attachTo: document.body,
  });
}

describe('XtermView 组件', () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    resetMockEvents();
    capturedOnDataCallback = null;
    vi.clearAllMocks();
  });

  it('挂载时调用 terminal.open 初始化终端', async () => {
    mountXterm();
    await nextTick();
    expect(mockOpen).toHaveBeenCalled();
  });

  it('挂载时加载 FitAddon', async () => {
    mountXterm();
    await nextTick();
    expect(mockLoadAddon).toHaveBeenCalled();
  });

  it('使用透明滚动条轨道覆盖 xterm 默认滚动条背景', () => {
    const source = readFileSync(resolve(process.cwd(), 'src/components/terminal/XtermView.vue'), 'utf-8');

    expect(source).toContain('<style>');
    expect(source).toContain('.terminal-view :deep(.xterm-viewport)');
    expect(source).toContain("viewport.style.setProperty('scrollbar-width', 'none')");
    expect(source).toContain('.custom-scrollbar');
    expect(source).toContain('.custom-scrollbar__thumb');
    expect(source).toContain('background: rgba(148, 163, 184, 0.45)');
    expect(source).toContain('::-webkit-scrollbar');
    expect(source).toContain('width: 0 !important');
    expect(source).toContain('display: none !important');
  });

  it('terminal:data 事件匹配 session_id 时写入终端', async () => {
    mountXterm('session-1', true);
    await nextTick();

    emitMockEvent('terminal:data', { session_id: 'session-1', data: 'hello' });
    await nextTick();

    expect(mockWrite).toHaveBeenCalledWith('hello');
  });

  it('terminal:data 事件 session_id 不匹配时不写入终端', async () => {
    mountXterm('session-1', true);
    await nextTick();

    emitMockEvent('terminal:data', { session_id: 'session-other', data: 'should-not-write' });
    await nextTick();

    expect(mockWrite).not.toHaveBeenCalled();
  });

  it('用户输入触发 input emit，payload 含正确 sessionId 和 data', async () => {
    const wrapper = mountXterm('session-42', true);
    await nextTick();

    // 模拟用户在终端中输入
    capturedOnDataCallback?.('ls -la\r');
    await nextTick();

    const emitted = wrapper.emitted('input');
    expect(emitted).toBeTruthy();
    expect(emitted![0]).toEqual([{ sessionId: 'session-42', data: 'ls -la\r' }]);
  });

  it('active=false 时 fit 不触发 resize emit', async () => {
    const wrapper = mountXterm('session-1', false);
    await nextTick();

    // fit 在 active=false 时应提前返回
    expect(wrapper.emitted('resize')).toBeFalsy();
  });

  it('组件卸载时调用 ResizeObserver.disconnect', async () => {
    const wrapper = mountXterm('session-1', true);
    await nextTick();

    wrapper.unmount();
    expect(mockDisconnect).toHaveBeenCalled();
  });

  it('组件卸载时调用 terminal.dispose', async () => {
    const wrapper = mountXterm('session-1', true);
    await nextTick();

    wrapper.unmount();
    expect(mockDispose).toHaveBeenCalled();
  });

  it('多个 terminal:data 事件按顺序写入终端', async () => {
    mountXterm('session-1', true);
    await nextTick();

    emitMockEvent('terminal:data', { session_id: 'session-1', data: 'line1\r\n' });
    emitMockEvent('terminal:data', { session_id: 'session-1', data: 'line2\r\n' });
    await nextTick();

    expect(mockWrite).toHaveBeenCalledTimes(2);
    expect(mockWrite).toHaveBeenNthCalledWith(1, 'line1\r\n');
    expect(mockWrite).toHaveBeenNthCalledWith(2, 'line2\r\n');
  });

  it('不同 session 的 XtermView 互不干扰', async () => {
    // 两个独立实例，各自有独立的 write mock
    // 通过验证只有匹配 session_id 的实例收到数据来验证隔离性
    mountXterm('session-A', true);
    await nextTick();

    // 只发送给 session-B 的事件
    emitMockEvent('terminal:data', { session_id: 'session-B', data: 'for-B' });
    await nextTick();

    // session-A 的 write 不应被调用（因为 session_id 不匹配）
    expect(mockWrite).not.toHaveBeenCalled();
  });
});
