import { invoke } from '@tauri-apps/api/core';
import { create } from 'zustand';
import { toAppError, type AppErrorInfo } from '@/i18n';

interface LogsState {
  /** 后端日志文件最近若干行（最新在末尾），查看器纯文本展示，不解析。 */
  lines: string[];
  /** 最近一次读取失败的结构化错误；读取失败保留旧行，绝不伪装成空列表。 */
  loadError: AppErrorInfo | null;
  /** 最近一次导出失败的结构化错误（与读取错误分开展示）。 */
  exportError: AppErrorInfo | null;
  load: () => Promise<void>;
  exportLogs: () => Promise<void>;
}

/** 轮询请求序号：只有最新发起的请求允许写入状态（latest-wins），
 *  迟到的慢响应/失败一律丢弃，不得用旧数据覆盖新响应。 */
let loadSeq = 0;

/** Settings“日志查看器”的视图投影：读取由挂载/轮询驱动，导出经原生保存对话框
 *  选定路径后复制后端日志文件。React 只消费 typed JSON（string[]），不解析日志内容。 */
export const useLogsStore = create<LogsState>((set) => ({
  lines: [],
  loadError: null,
  exportError: null,
  async load() {
    const seq = ++loadSeq;
    try {
      const lines = await invoke<string[]>('get_recent_logs');
      if (seq !== loadSeq) return;
      if (!Array.isArray(lines)) {
        // IPC 边界防御：后端契约是 string[]，非数组 payload 视为读取失败，
        // 保留旧行并暴露错误，绝不伪装成空列表
        set({ loadError: { code: 'Unknown', detail: 'Invalid log payload (get_recent_logs)' } });
        return;
      }
      set({ lines, loadError: null });
    } catch (error) {
      if (seq !== loadSeq) return;
      set({ loadError: toAppError(error) });
    }
  },
  async exportLogs() {
    try {
      await invoke('export_logs');
      set({ exportError: null });
    } catch (error) {
      set({ exportError: toAppError(error) });
    }
  },
}));
