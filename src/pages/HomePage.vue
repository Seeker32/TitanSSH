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
import type { HostConfig, SaveHostRequest } from '@/types/host';

const hostStore = useHostStore();
const sessionStore = useSessionStore();
const monitorStore = useMonitorStore();
const themeStore = useThemeStore();

const editorVisible = ref(false);
const editingHost = ref<HostConfig | null>(null);
const activeHostId = ref<string | null>(null);
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

async function openSession(hostId: string) {
  activeHostId.value = hostId;
  await sessionStore.openSession(hostId);
}

function createHost() {
  editingHost.value = null;
  editorVisible.value = true;
}

function editHost(host: HostConfig) {
  editingHost.value = host;
  editorVisible.value = true;
}

async function saveHost(host: SaveHostRequest) {
  await hostStore.saveHost(host);
  editorVisible.value = false;
  editingHost.value = null;
}

async function removeHost(hostId: string) {
  await hostStore.deleteHost(hostId);
  if (activeHostId.value === hostId) {
    activeHostId.value = null;
  }
}

onMounted(async () => {
  disposeSessionListeners = await sessionStore.initListeners();
  disposeMonitorListeners = await monitorStore.initListeners();
  await hostStore.loadHosts();
});

onBeforeUnmount(() => {
  disposeSessionListeners?.();
  disposeMonitorListeners?.();
});
</script>

<template>
  <div class="page-shell">
    <aside class="sidebar">
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
  width: 300px;
  min-width: 300px;
  height: 100%;
  padding: 16px;
  gap: 12px;
  border-right: 1px solid var(--color-border);
  background: var(--color-panel-bg);
}

.sidebar-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  flex-shrink: 0;
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

@media (max-width: 1080px) {
  .sidebar {
    width: 260px;
    min-width: 260px;
  }
}

@media (max-width: 860px) {
  .page-shell {
    flex-direction: column;
  }

  .sidebar {
    width: 100%;
    height: auto;
    max-height: 40vh;
    border-right: none;
    border-bottom: 1px solid var(--color-border);
  }

  .main-panel {
    flex: 1;
  }
}
</style>
