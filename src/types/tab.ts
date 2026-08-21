/**
 * 标签视图模型（ADR-0002）：标签是纯视图，引用会话但不拥有连接；
 * 终端标签是会话锚点（关闭即触发完整 teardown），后续视图类型（如进程标签）扩展 TabType。
 */

/** 标签类型：terminal = 会话锚点；进程标签等纯视图类型在后续版本扩展此联合 */
export type TabType = 'terminal';

/** 标签视图实体：不拥有连接，仅引用所属 Runtime Session；标签栏按插入顺序渲染 */
export interface TerminalTab {
  tabId: string;
  type: TabType;
  /** 引用的 Runtime Session；连接生命周期归 SessionManager，标签只是视图锚点 */
  sessionId: string;
  /** Unix 毫秒时间戳 */
  createdAt: number;
}

/** 构造终端标签的确定性 ID：一个会话恰有一个终端标签，按 sessionId 派生保证唯一 */
export function terminalTabId(sessionId: string): string {
  return `terminal:${sessionId}`;
}
