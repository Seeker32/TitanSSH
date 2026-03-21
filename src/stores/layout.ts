import { computed, ref } from 'vue';
import { defineStore } from 'pinia';

export const DEFAULT_SIDEBAR_WIDTH = 300;
export const MIN_SIDEBAR_WIDTH = 220;
export const MAX_SIDEBAR_WIDTH = 520;
export const MIN_MAIN_PANEL_WIDTH = 480;

/** 根据视口宽度计算并限制侧栏宽度，确保主内容区保留最小可用空间。 */
function clampSidebarWidth(width: number, viewportWidth: number) {
  const maxAllowedWidth = Math.min(MAX_SIDEBAR_WIDTH, Math.max(MIN_SIDEBAR_WIDTH, viewportWidth - MIN_MAIN_PANEL_WIDTH));
  return Math.min(Math.max(width, MIN_SIDEBAR_WIDTH), maxAllowedWidth);
}

export const useLayoutStore = defineStore('layout', () => {
  const sidebarWidth = ref(DEFAULT_SIDEBAR_WIDTH);

  const sidebarMaxWidth = computed(() => clampSidebarWidth(MAX_SIDEBAR_WIDTH, window.innerWidth));

  /** 设置左侧栏宽度，并根据当前视口宽度限制最小值与最大值。 */
  function setSidebarWidth(width: number) {
    sidebarWidth.value = clampSidebarWidth(width, window.innerWidth);
  }

  /** 在窗口尺寸变化后重新同步左侧栏宽度，避免主内容区被挤压。 */
  function syncSidebarWidthForViewport(viewportWidth: number) {
    sidebarWidth.value = clampSidebarWidth(sidebarWidth.value, viewportWidth);
  }

  return {
    sidebarWidth,
    sidebarMaxWidth,
    setSidebarWidth,
    syncSidebarWidthForViewport,
  };
});
