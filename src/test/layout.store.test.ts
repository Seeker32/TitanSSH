import { beforeEach, describe, expect, it } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import {
  DEFAULT_SIDEBAR_WIDTH,
  MAX_SIDEBAR_WIDTH,
  MIN_MAIN_PANEL_WIDTH,
  MIN_SIDEBAR_WIDTH,
  useLayoutStore,
} from '@/stores/layout';

function setViewportWidth(width: number) {
  Object.defineProperty(window, 'innerWidth', {
    configurable: true,
    writable: true,
    value: width,
  });
}

describe('layout store', () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    setViewportWidth(1280);
  });

  it('uses the default sidebar width on initialization', () => {
    const store = useLayoutStore();

    expect(store.sidebarWidth).toBe(DEFAULT_SIDEBAR_WIDTH);
  });

  it('clamps sidebar width to the minimum width', () => {
    const store = useLayoutStore();

    store.setSidebarWidth(MIN_SIDEBAR_WIDTH - 80);

    expect(store.sidebarWidth).toBe(MIN_SIDEBAR_WIDTH);
  });

  it('clamps sidebar width to the computed maximum width', () => {
    const store = useLayoutStore();

    store.setSidebarWidth(MAX_SIDEBAR_WIDTH + 200);

    expect(store.sidebarWidth).toBe(MAX_SIDEBAR_WIDTH);
  });

  it('re-clamps the current width when viewport shrinks', () => {
    const store = useLayoutStore();
    store.setSidebarWidth(500);
    setViewportWidth(760);

    store.syncSidebarWidthForViewport(window.innerWidth);

    expect(store.sidebarWidth).toBe(760 - MIN_MAIN_PANEL_WIDTH);
  });
});
