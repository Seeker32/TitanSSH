<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from 'vue';
import HostEditorDialog from '@/components/host/HostEditorDialog.vue';
import HostList from '@/components/host/HostList.vue';
import TerminalPane from '@/components/terminal/TerminalPane.vue';
import TerminalTabs from '@/components/terminal/TerminalTabs.vue';
import { useHostStore } from '@/stores/host';
import { useMonitorStore } from '@/stores/monitor';
import { useSessionStore } from '@/stores/session';
import { useThemeStore } from '@/stores/theme';
import type { HostConfig } from '@/types/host';

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
  const session = await sessionStore.openSession(hostId);
  await monitorStore.fetchStatus(session.session_id).catch(() => undefined);
}

function createHost() {
  editingHost.value = null;
  editorVisible.value = true;
}

function editHost(host: HostConfig) {
  editingHost.value = host;
  editorVisible.value = true;
}

async function saveHost(host: HostConfig) {
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
  sessionStore.initHomeSession();
});

onBeforeUnmount(() => {
  disposeSessionListeners?.();
  disposeMonitorListeners?.();
});
</script>

<template>
  <div class="page-shell">
    <aside class="sidebar">
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
      <button class="theme-toggle" @click="themeStore.toggleTheme()">
        <span v-if="themeStore.theme === 'dark'" class="theme-icon">暗</span>
        <span v-else class="theme-icon">亮</span>
      </button>
    </aside>

    <section class="main-panel">
      <div class="tabs-area">
        <TerminalTabs
          :sessions="sessionStore.sessionList"
          :active-session-id="sessionStore.activeSessionId"
          @activate="sessionStore.setActiveSession"
          @close="sessionStore.closeSession"
        />
      </div>
      <div class="content-area">
        <TerminalPane
          :sessions="sessionStore.sessionList"
          :active-session-id="sessionStore.activeSessionId"
          :outputs="sessionStore.terminalOutput"
          :hosts="hostStore.hosts"
          @activate="sessionStore.setActiveSession"
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
  width: 320px;
  min-width: 320px;
  height: 100%;
  padding: 18px;
  border-right: 1px solid var(--color-border);
  background: var(--color-panel-bg);
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
  height: 50px;
  min-height: 50px;
  padding: 8px 18px 0;
  border-bottom: 1px solid var(--color-border);
  background: var(--color-panel-bg);
  overflow: hidden;
}

.content-area {
  flex: 1;
  min-height: 0;
  padding: 18px;
  overflow: hidden;
}

.theme-toggle {
  margin-top: auto;
  padding: 12px 16px;
  border: 1px solid var(--color-border);
  border-radius: 12px;
  color: var(--color-text-secondary);
  background: var(--color-card-bg);
  cursor: pointer;
  transition: all 0.2s ease;
  font-size: 14px;
  text-align: center;
}

.theme-toggle:hover {
  color: var(--color-text-primary);
  background: var(--color-card-bg-hover);
  border-color: var(--color-border-focus);
}

.theme-icon {
  font-size: 14px;
}

@media (max-width: 1080px) {
  .sidebar {
    width: 280px;
    min-width: 280px;
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
