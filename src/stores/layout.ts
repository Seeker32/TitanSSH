import { create } from 'zustand';

export const DEFAULT_SIDEBAR_WIDTH = 300;
export const MIN_SIDEBAR_WIDTH = 220;
export const MAX_SIDEBAR_WIDTH = 520;
export const MIN_MAIN_PANEL_WIDTH = 480;

/** 根据视口宽度限制侧栏宽度，确保主内容区可用。 */
export function clampSidebarWidth(width: number, viewportWidth: number) {
  const max = Math.min(MAX_SIDEBAR_WIDTH, Math.max(MIN_SIDEBAR_WIDTH, viewportWidth - MIN_MAIN_PANEL_WIDTH));
  return Math.min(Math.max(width, MIN_SIDEBAR_WIDTH), max);
}

interface LayoutState {
  sidebarWidth: number;
  setSidebarWidth: (width: number) => void;
  syncSidebarWidthForViewport: (viewportWidth: number) => void;
}

export const useLayoutStore = create<LayoutState>((set, get) => ({
  sidebarWidth: DEFAULT_SIDEBAR_WIDTH,

  /** 设置侧栏宽度，并按当前视口限制范围。 */
  setSidebarWidth(width) {
    set({ sidebarWidth: clampSidebarWidth(width, window.innerWidth) });
  },

  /** 窗口变化后重新限制当前侧栏宽度。 */
  syncSidebarWidthForViewport(viewportWidth) {
    set({ sidebarWidth: clampSidebarWidth(get().sidebarWidth, viewportWidth) });
  },
}));
