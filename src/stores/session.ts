import { defineStore } from 'pinia';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { computed, ref } from 'vue';
import type { SessionInfo } from '@/types/session';
import { SessionStatus } from '@/types/session';
import type { MonitorSnapshot } from '@/types/monitor';
import { useMonitorStore } from './monitor';

/** session:status 事件 payload */
interface SessionStatusPayload {
  session_id: string;
  status: SessionStatus;
  message?: string | null;
}

/** terminal:data 事件 payload */
interface TerminalDataPayload {
  session_id: string;
  data: string;
}

export const useSessionStore = defineStore('session', () => {
  /** 真实 SSH 会话集合，不含首页或占位项 */
  const sessions = ref(new Map<string, SessionInfo>());

  /**
   * 当前激活视图 ID：'home' 表示首页，其他值为真实 session_id
   * 首页是前端固定视图，不属于后端 Session
   */
  const activeView = ref<'home' | string>('home');

  /** 监控快照集合，按 session_id 索引 */
  const snapshots = ref(new Map<string, MonitorSnapshot>());

  /** 状态栏消息 */
  const statusMessage = ref<string>('就绪');

  /** 所有真实会话列表（不含首页） */
  const sessionList = computed(() => Array.from(sessions.value.values()));

  /** 当前激活的真实会话，首页激活时返回 null */
  const activeSession = computed(() =>
    activeView.value !== 'home' ? (sessions.value.get(activeView.value) ?? null) : null,
  );

  /** 当前激活会话的监控快照 */
  const activeSnapshot = computed(() =>
    activeView.value !== 'home' ? (snapshots.value.get(activeView.value) ?? null) : null,
  );

  /** 打开新的 SSH 会话，返回后端创建的 SessionInfo，并自动启动监控任务 */
  async function openSession(hostId: string) {
    const session = await invoke<SessionInfo>('open_session', { hostId });
    sessions.value = new Map(sessions.value).set(session.session_id, session);
    activeView.value = session.session_id;
    statusMessage.value = `正在连接 ${session.username}@${session.host}`;
    // 会话建立后立即启动监控任务，失败时静默处理不影响主流程
    try {
      const monitorStore = useMonitorStore();
      await monitorStore.startMonitoring(session.session_id);
    } catch {
      // 监控启动失败不阻断会话主流程
    }
    return session;
  }

  /** 关闭指定会话，先停止监控任务，若关闭的是当前激活会话则回退到首页视图 */
  async function closeSession(sessionId: string) {
    // 关闭会话前先停止监控任务，失败时静默处理
    try {
      const monitorStore = useMonitorStore();
      await monitorStore.stopMonitoring(sessionId);
    } catch {
      // 监控停止失败不阻断关闭流程
    }
    await invoke('close_session', { sessionId });
    const next = new Map(sessions.value);
    next.delete(sessionId);
    sessions.value = next;

    const nextSnapshots = new Map(snapshots.value);
    nextSnapshots.delete(sessionId);
    snapshots.value = nextSnapshots;

    if (activeView.value === sessionId) {
      activeView.value = 'home';
    }
  }

  /** 向指定会话的终端写入数据 */
  async function writeTerminal(sessionId: string, data: string) {
    await invoke('write_terminal', { sessionId, data });
  }

  /** 同步终端尺寸到后端 PTY */
  async function resizeTerminal(sessionId: string, cols: number, rows: number) {
    await invoke('resize_terminal', { sessionId, cols, rows });
  }

  /** 切换激活视图，'home' 表示首页，其他值为 session_id */
  function setActiveView(viewId: 'home' | string) {
    activeView.value = viewId;
  }

  /** 处理 session:status 事件，更新会话状态与状态栏消息，并同步后端元数据 */
  function applySessionStatus(payload: SessionStatusPayload) {
    const current = sessions.value.get(payload.session_id);
    if (current) {
      sessions.value = new Map(sessions.value).set(payload.session_id, {
        ...current,
        status: payload.status,
      });
    }
    statusMessage.value = statusLabel(payload.status, payload.message ?? undefined);
    // 同步状态到后端 SessionManager 元数据，修复 P1-1（list_sessions 状态不可靠）
    try {
      const result = invoke('sync_session_status', {
        sessionId: payload.session_id,
        status: payload.status,
      });
      if (result && typeof (result as Promise<void>).catch === 'function') {
        (result as Promise<void>).catch(() => {});
      }
    } catch {
      // 同步失败静默处理
    }
  }

  /** 处理 monitor:snapshot 事件，更新对应会话的监控快照 */
  function applySnapshot(snapshot: MonitorSnapshot) {
    snapshots.value = new Map(snapshots.value).set(snapshot.session_id, snapshot);
  }

  /** 将 SessionStatus 枚举值转换为对应的中文状态提示文本
   * 错误类状态（AuthFailed、Timeout、Error、Disconnected）返回详细中文提示
   * Error 状态若后端提供了 message 则拼接到提示文本中
   */
  function statusLabel(status: SessionStatus, message?: string): string {
    switch (status) {
      case SessionStatus.Connecting:
        return '正在连接...';
      case SessionStatus.Connected:
        return '';
      case SessionStatus.AuthFailed:
        return '认证失败，请检查用户名和密码';
      case SessionStatus.Timeout:
        return '连接超时，请检查网络或主机地址';
      case SessionStatus.Error:
        return message?.trim() ? `连接错误：${message.trim()}` : '连接错误';
      case SessionStatus.Disconnected:
        return '连接已断开';
      default:
        return '连接异常';
    }
  }

  /**
   * 初始化后端事件监听器：
   * - session:status：会话状态变更
   * - terminal:data：终端数据流（不在 store 中缓冲，仅供 XtermView 直接消费）
   * - monitor:snapshot：同步更新本 store 的快照缓存（monitorStore 为主，此处为兼容层）
   * 返回取消监听的清理函数
   */
  async function initListeners() {
    const unlistenStatus = await listen<SessionStatusPayload>('session:status', (event) => {
      applySessionStatus(event.payload);
    });

    // terminal:data 不在 store 中缓冲，XtermView 组件自行监听此事件流
    const unlistenData = await listen<TerminalDataPayload>('terminal:data', (_event) => {});

    const unlistenSnapshot = await listen<MonitorSnapshot>('monitor:snapshot', (event) => {
      applySnapshot(event.payload);
    });

    return () => {
      unlistenStatus();
      unlistenData();
      unlistenSnapshot();
    };
  }

  return {
    sessions,
    activeView,
    snapshots,
    sessionList,
    activeSession,
    activeSnapshot,
    statusMessage,
    openSession,
    closeSession,
    writeTerminal,
    resizeTerminal,
    setActiveView,
    applySessionStatus,
    applySnapshot,
    initListeners,
  };
});
