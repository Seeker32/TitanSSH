import { defineStore } from 'pinia';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { computed, ref } from 'vue';
import type { SessionInfo, SessionStatus } from '@/types/session';

interface SessionStatusPayload {
  session_id: string;
  status: SessionStatus;
  message?: string | null;
}

interface TerminalDataPayload {
  session_id: string;
  data: string;
}

export const useSessionStore = defineStore('session', () => {
  const HOME_SESSION_ID = '__home__';
  
  const sessions = ref(new Map<string, SessionInfo>());
  const activeSessionId = ref<string | null>(null);
  const terminalOutput = ref(new Map<string, string>());
  const statusMessage = ref<string>('就绪');

  // Initialize home session
  function initHomeSession() {
    if (!sessions.value.has(HOME_SESSION_ID)) {
      const homeSession: SessionInfo = {
        session_id: HOME_SESSION_ID,
        host_id: '',
        host: '首页',
        port: 0,
        username: '',
        status: 'Connected' as SessionStatus,
        created_at: Date.now(),
        active: false,
        isHome: true,
      };
      sessions.value = new Map(sessions.value).set(HOME_SESSION_ID, homeSession);
      if (!activeSessionId.value) {
        activeSessionId.value = HOME_SESSION_ID;
      }
    }
  }

  const sessionList = computed(() => Array.from(sessions.value.values()));
  const activeSession = computed(() =>
    activeSessionId.value ? sessions.value.get(activeSessionId.value) ?? null : null,
  );

  async function openSession(hostId: string) {
    const session = await invoke<SessionInfo>('open_session', { hostId });
    sessions.value = new Map(sessions.value).set(session.session_id, session);
    if (!terminalOutput.value.has(session.session_id)) {
      terminalOutput.value = new Map(terminalOutput.value).set(session.session_id, '');
    }
    activeSessionId.value = session.session_id;
    statusMessage.value = `正在连接 ${session.username}@${session.host}`;
    return session;
  }

  async function closeSession(sessionId: string) {
    // Prevent closing home session
    const session = sessions.value.get(sessionId);
    if (session?.isHome) {
      return;
    }
    
    await invoke('close_session', { sessionId });
    const nextSessions = new Map(sessions.value);
    nextSessions.delete(sessionId);
    sessions.value = nextSessions;

    const nextOutput = new Map(terminalOutput.value);
    nextOutput.delete(sessionId);
    terminalOutput.value = nextOutput;

    if (activeSessionId.value === sessionId) {
      activeSessionId.value = HOME_SESSION_ID;
    }
  }

  async function writeTerminal(sessionId: string, data: string) {
    await invoke('write_terminal', { sessionId, data });
  }

  async function resizeTerminal(sessionId: string, cols: number, rows: number) {
    await invoke('resize_terminal', { sessionId, cols, rows });
  }

  function setActiveSession(sessionId: string) {
    activeSessionId.value = sessionId;
  }

  function getOutput(sessionId: string) {
    return terminalOutput.value.get(sessionId) ?? '';
  }

  function applySessionStatus(payload: SessionStatusPayload) {
    const current = sessions.value.get(payload.session_id);
    if (current) {
      sessions.value = new Map(sessions.value).set(payload.session_id, {
        ...current,
        status: payload.status,
        active: payload.status === 'Connected',
      });
    }
    statusMessage.value = payload.message?.trim() || statusLabel(payload.status);
  }

  function appendTerminalData(payload: TerminalDataPayload) {
    terminalOutput.value = new Map(terminalOutput.value).set(
      payload.session_id,
      `${terminalOutput.value.get(payload.session_id) ?? ''}${payload.data}`,
    );
  }

  async function initListeners() {
    const unlistenStatus = await listen<SessionStatusPayload>('session:status', (event) => {
      applySessionStatus(event.payload);
    });
    const unlistenData = await listen<TerminalDataPayload>('terminal:data', (event) => {
      appendTerminalData(event.payload);
    });

    return () => {
      unlistenStatus();
      unlistenData();
    };
  }

  function statusLabel(status: SessionStatus) {
    switch (status) {
      case 'Connecting':
        return '连接中';
      case 'Connected':
        return '已连接';
      case 'AuthFailed':
        return '认证失败';
      case 'Timeout':
        return '连接超时';
      case 'Disconnected':
        return '已断开';
      default:
        return '连接异常';
    }
  }

  return {
    sessions,
    activeSessionId,
    terminalOutput,
    sessionList,
    activeSession,
    statusMessage,
    openSession,
    closeSession,
    writeTerminal,
    resizeTerminal,
    setActiveSession,
    getOutput,
    initListeners,
    initHomeSession,
    HOME_SESSION_ID,
  };
});
