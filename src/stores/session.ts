import { defineStore } from 'pinia';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { computed, ref } from 'vue';
import type { SessionInfo, SessionProgressEvent } from '@/types/session';
import { ConnectionPhase, SessionStatus } from '@/types/session';
import type { MonitorSnapshot } from '@/types/monitor';
import { useMonitorStore } from './monitor';

const CONNECT_WATCHDOG_MS = 15_000;

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
  /** 连接阶段 watchdog 定时器，避免后端阻塞时 UI 永久停留在“连接中” */
  const connectWatchdogs = new Map<string, ReturnType<typeof setTimeout>>();

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
    scheduleConnectWatchdog(session.session_id);
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
    clearConnectWatchdog(sessionId);
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
    if (payload.status !== SessionStatus.Connecting) {
      clearConnectWatchdog(payload.session_id);
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

  /** 处理 session:progress 事件，仅在会话仍处于 Connecting 时更新状态栏诊断信息 */
  function applySessionProgress(payload: SessionProgressEvent) {
    const current = sessions.value.get(payload.sessionId);
    if (!current || current.status !== SessionStatus.Connecting) {
      return;
    }
    statusMessage.value = progressLabel(payload.phase, payload.message);
  }

  /** 为指定会话注册连接超时 watchdog，防止系统钥匙串等阻塞导致前端永久卡在连接中 */
  function scheduleConnectWatchdog(sessionId: string) {
    clearConnectWatchdog(sessionId);
    connectWatchdogs.set(
      sessionId,
      setTimeout(() => {
        const current = sessions.value.get(sessionId);
        if (!current || current.status !== SessionStatus.Connecting) {
          return;
        }
        applySessionStatus({
          session_id: sessionId,
          status: SessionStatus.Timeout,
          message: `Connection watchdog timeout after ${CONNECT_WATCHDOG_MS / 1000}s`,
        });
      }, CONNECT_WATCHDOG_MS),
    );
  }

  /** 清理指定会话的连接 watchdog，避免终态后重复触发超时收敛 */
  function clearConnectWatchdog(sessionId: string) {
    const timer = connectWatchdogs.get(sessionId);
    if (!timer) {
      return;
    }
    clearTimeout(timer);
    connectWatchdogs.delete(sessionId);
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

  /** 将连接阶段转换为用户可读的中文进度文本，优先使用后端附带文案 */
  function progressLabel(phase: ConnectionPhase, message?: string): string {
    if (message?.trim()) {
      return message.trim();
    }

    switch (phase) {
      case ConnectionPhase.LoadingCredentials:
        return '正在读取凭据...';
      case ConnectionPhase.ConnectingTcp:
        return '正在建立 TCP 连接...';
      case ConnectionPhase.SshHandshake:
        return '正在进行 SSH 握手...';
      case ConnectionPhase.Authenticating:
        return '正在进行 SSH 认证...';
      case ConnectionPhase.OpeningChannel:
        return '正在打开终端通道...';
      case ConnectionPhase.RequestingPty:
        return '正在请求终端 PTY...';
      case ConnectionPhase.StartingShell:
        return '正在启动 Shell...';
      default:
        return '正在连接...';
    }
  }

  /**
   * 初始化后端事件监听器：
   * - session:status：会话状态变更
   * - session:progress：连接阶段诊断进度
   * - terminal:data：终端数据流（不在 store 中缓冲，仅供 XtermView 直接消费）
   * - monitor:snapshot：同步更新本 store 的快照缓存（monitorStore 为主，此处为兼容层）
   * 返回取消监听的清理函数
   */
  async function initListeners() {
    const unlistenStatus = await listen<SessionStatusPayload>('session:status', (event) => {
      console.log('[diagnostic] session:status received:', event.payload);
      applySessionStatus(event.payload);
    });

    const unlistenProgress = await listen<SessionProgressEvent>('session:progress', (event) => {
      applySessionProgress(event.payload);
    });

    // terminal:data 不在 store 中缓冲，XtermView 组件自行监听此事件流
    const unlistenData = await listen<TerminalDataPayload>('terminal:data', (_event) => {});

    const unlistenSnapshot = await listen<MonitorSnapshot>('monitor:snapshot', (event) => {
      applySnapshot(event.payload);
    });

    return () => {
      unlistenStatus();
      unlistenProgress();
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
    applySessionProgress,
    applySnapshot,
    initListeners,
  };
});
