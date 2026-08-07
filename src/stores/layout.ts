import { create } from 'zustand';

export const DEFAULT_SIDEBAR_WIDTH = 300;
export const MIN_SIDEBAR_WIDTH = 220;
export const MAX_SIDEBAR_WIDTH = 520;
export const MIN_MAIN_PANEL_WIDTH = 480;

/** 分组折叠状态在本地存储中的键 */
const COLLAPSED_GROUPS_KEY = 'collapsed-groups';

/** 从本地存储读取折叠的分组名列表；损坏或缺失时返回空列表。 */
export function readCollapsedGroups(): string[] {
  try {
    const raw = localStorage.getItem(COLLAPSED_GROUPS_KEY);
    if (!raw) return [];
    const parsed = JSON.parse(raw);
    return Array.isArray(parsed) ? parsed.filter((name): name is string => typeof name === 'string') : [];
  } catch {
    return [];
  }
}

/** 根据视口宽度限制侧栏宽度，确保主内容区可用。 */
export function clampSidebarWidth(width: number, viewportWidth: number) {
  const max = Math.min(MAX_SIDEBAR_WIDTH, Math.max(MIN_SIDEBAR_WIDTH, viewportWidth - MIN_MAIN_PANEL_WIDTH));
  return Math.min(Math.max(width, MIN_SIDEBAR_WIDTH), max);
}

interface LayoutState {
  sidebarWidth: number;
  /** 折叠的分组名列表（空串 = "未分组"） */
  collapsedGroups: string[];
  setSidebarWidth: (width: number) => void;
  syncSidebarWidthForViewport: (viewportWidth: number) => void;
  toggleGroupCollapsed: (name: string) => void;
}

export const useLayoutStore = create<LayoutState>((set, get) => ({
  sidebarWidth: DEFAULT_SIDEBAR_WIDTH,
  collapsedGroups: readCollapsedGroups(),

  /** 设置侧栏宽度，并按当前视口限制范围。 */
  setSidebarWidth(width) {
    set({ sidebarWidth: clampSidebarWidth(width, window.innerWidth) });
  },

  /** 窗口变化后重新限制当前侧栏宽度。 */
  syncSidebarWidthForViewport(viewportWidth) {
    set({ sidebarWidth: clampSidebarWidth(get().sidebarWidth, viewportWidth) });
  },

  /** 切换分组折叠状态并持久化到本地存储。 */
  toggleGroupCollapsed(name) {
    const next = get().collapsedGroups.includes(name)
      ? get().collapsedGroups.filter((item) => item !== name)
      : [...get().collapsedGroups, name];
    localStorage.setItem(COLLAPSED_GROUPS_KEY, JSON.stringify(next));
    set({ collapsedGroups: next });
  },
}));
