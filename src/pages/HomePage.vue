<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from 'vue';
import { NButton, NText } from 'naive-ui';
import HostEditorDialog from '@/components/host/HostEditorDialog.vue';
import HostList from '@/components/host/HostList.vue';
import TerminalPane from '@/components/terminal/TerminalPane.vue';
import TerminalTabs from '@/components/terminal/TerminalTabs.vue';
import { useHostStore } from '@/stores/host';
import { useMonitorStore } from '@/stores/monitor';
import { useSessionStore } from '@/stores/session';
import { useThemeStore } from '@/stores/theme';
import { useLayoutStore } from '@/stores/layout';
import type { HostConfig, SaveHostRequest } from '@/types/host';

const hostStore = useHostStore();
const sessionStore = useSessionStore();
const monitorStore = useMonitorStore();
const themeStore = useThemeStore();
const layoutStore = useLayoutStore();

const editorVisible = ref(false);
const editingHost = ref<HostConfig | null>(null);
const activeHostId = ref<string | null>(null);
const isResizingSidebar = ref(false);
let disposeSessionListeners: (() => void) | null = null;
let disposeMonitorListeners: (() => void) | null = null;

const connectedHostIds = computed(() =>
  sessionStore.sessionList
    .filter((session) => session.status === 'Connected')
    .map((session) => session.host_id),
);

const connectingHostIds = computed(() =>
  sessionStore.sessionList
    .filter((session) => session.status === 'Connecting')
    .map((session) => session.host_id),
);

/** 打开指定主机的 SSH 会话，并同步当前激活主机。 */
async function openSession(hostId: string) {
  activeHostId.value = hostId;
  await sessionStore.openSession(hostId);
}

/** 打开新建主机对话框，并清空当前编辑对象。 */
function createHost() {
  editingHost.value = null;
  editorVisible.value = true;
}

/** 打开编辑主机对话框，并注入待编辑主机数据。 */
function editHost(host: HostConfig) {
  editingHost.value = host;
  editorVisible.value = true;
}

/** 保存主机配置，并在成功后关闭编辑对话框。 */
async function saveHost(host: SaveHostRequest) {
  await hostStore.saveHost(host);
  editorVisible.value = false;
  editingHost.value = null;
}

/** 删除主机配置；若删除的是当前激活主机则同步清空激活态。 */
async function removeHost(hostId: string) {
  await hostStore.deleteHost(hostId);
  if (activeHostId.value === hostId) {
    activeHostId.value = null;
  }
}

/** 根据鼠标横坐标更新左侧栏宽度，并确保宽度在合法范围内。 */
function updateSidebarWidth(clientX: number) {
  layoutStore.setSidebarWidth(clientX);
}

/** 处理拖拽过程中的指针移动事件，实时刷新左侧栏宽度。 */
function handleSidebarPointerMove(event: PointerEvent) {
  if (!isResizingSidebar.value) {
    return;
  }

  updateSidebarWidth(event.clientX);
}

/** 结束左侧栏拖拽，并清理全局拖拽状态。 */
function stopSidebarResize() {
  if (!isResizingSidebar.value) {
    return;
  }

  isResizingSidebar.value = false;
  document.body.classList.remove('sidebar-resizing');
}

/** 开始左侧栏拖拽，并绑定当前指针位置为宽度基准。 */
function startSidebarResize(event: PointerEvent) {
  isResizingSidebar.value = true;
  document.body.classList.add('sidebar-resizing');
  updateSidebarWidth(event.clientX);
}

/** 在窗口尺寸变化时同步侧栏宽度，避免布局溢出。 */
function handleWindowResize() {
  layoutStore.syncSidebarWidthForViewport(window.innerWidth);
}

onMounted(async () => {
  window.addEventListener('pointermove', handleSidebarPointerMove);
  window.addEventListener('pointerup', stopSidebarResize);
  window.addEventListener('resize', handleWindowResize);
  handleWindowResize();
  disposeSessionListeners = await sessionStore.initListeners();
  disposeMonitorListeners = await monitorStore.initListeners();
  await hostStore.loadHosts();
});

onBeforeUnmount(() => {
  window.removeEventListener('pointermove', handleSidebarPointerMove);
  window.removeEventListener('pointerup', stopSidebarResize);
  window.removeEventListener('resize', handleWindowResize);
  stopSidebarResize();
  disposeSessionListeners?.();
  disposeMonitorListeners?.();
});
</script>

<template>
  <div class="page-shell" :class="{ 'page-shell--resizing': isResizingSidebar }">
    <aside class="sidebar" :style="{ width: `${layoutStore.sidebarWidth}px` }">
      <div class="sidebar-header">
        <NText depth="3" style="font-size: 11px; letter-spacing: 0.12em; text-transform: uppercase">
          Titan SSH
        </NText>
        <NButton text size="small" @click="themeStore.toggleTheme()">
          {{ themeStore.theme === 'dark' ? '🌙' : '☀️' }}
        </NButton>
      </div>
      <HostList
        :hosts="hostStore.hosts"
        :active-host-id="activeHostId"
        :connected-host-ids="connectedHostIds"
        :connecting-host-ids="connectingHostIds"
        @open="openSession"
        @create="createHost"
        @edit="editHost"
        @remove="removeHost"
      />
    </aside>
    <div
      class="sidebar-resizer"
      role="separator"
      aria-orientation="vertical"
      :aria-valuenow="layoutStore.sidebarWidth"
      :aria-valuemin="220"
      :aria-valuemax="layoutStore.sidebarMaxWidth"
      @pointerdown="startSidebarResize"
    />

    <section class="main-panel">
      <div class="tabs-area">
        <TerminalTabs
          :sessions="sessionStore.sessionList"
          :active-view="sessionStore.activeView"
          @activate="sessionStore.setActiveView"
          @close="sessionStore.closeSession"
        />
      </div>
      <div class="content-area">
        <TerminalPane
          :sessions="sessionStore.sessionList"
          :active-view="sessionStore.activeView"
          :hosts="hostStore.hosts"
          @activate="sessionStore.setActiveView"
          @close="sessionStore.closeSession"
          @input="sessionStore.writeTerminal($event.sessionId, $event.data)"
          @resize="sessionStore.resizeTerminal($event.sessionId, $event.cols, $event.rows)"
          @open-host="openSession"
          @create-host="createHost"
        />
      </div>
    </section>

    <HostEditorDialog
      v-model="editorVisible"
      :editing-host="editingHost"
      @save="saveHost"
    />
  </div>
</template>

<style scoped>
.page-shell {
  display: flex;
  height: 100vh;
  overflow: hidden;
}

.sidebar {
  display: flex;
  flex-direction: column;
  min-width: 220px;
  height: 100%;
  padding: 16px;
  gap: 12px;
  border-right: 1px solid var(--color-border);
  background: var(--color-panel-bg);
  flex-shrink: 0;
}

.sidebar-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  flex-shrink: 0;
}

.sidebar-resizer {
  position: relative;
  width: 8px;
  flex-shrink: 0;
  cursor: col-resize;
  background: transparent;
}

.sidebar-resizer::before {
  content: '';
  position: absolute;
  top: 0;
  bottom: 0;
  left: 3px;
  width: 2px;
  border-radius: 999px;
  background: var(--color-border);
  transition: background 0.2s ease;
}

.sidebar-resizer:hover::before,
.page-shell--resizing .sidebar-resizer::before {
  background: var(--color-accent);
}

.main-panel {
  display: flex;
  flex-direction: column;
  flex: 1;
  min-width: 0;
  height: 100%;
  overflow: hidden;
}

.tabs-area {
  height: 42px;
  min-height: 42px;
  padding: 0;
  border-bottom: 1px solid var(--color-border);
  background: var(--color-panel-bg);
  overflow: hidden;
}

.content-area {
  flex: 1;
  min-height: 0;
  overflow: hidden;
}

.page-shell--resizing {
  user-select: none;
}

@media (max-width: 1080px) {
  .sidebar {
    padding-right: 12px;
  }
}
</style>
