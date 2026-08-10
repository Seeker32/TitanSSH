import { invoke } from '@tauri-apps/api/core';
import { create } from 'zustand';

export const LOG_LEVELS = ['error', 'warn', 'info', 'debug', 'trace'] as const;
export type LogLevel = typeof LOG_LEVELS[number];

const LOG_LEVEL_KEY = 'log-level';

/** 读取已保存的日志等级；无效或不可用的本地存储回退为 info。 */
export function readLogLevel(): LogLevel {
  try {
    const saved = localStorage.getItem(LOG_LEVEL_KEY);
    return LOG_LEVELS.includes(saved as LogLevel) ? saved as LogLevel : 'info';
  } catch {
    return 'info';
  }
}

interface LogLevelState {
  logLevel: LogLevel;
  setLogLevel: (logLevel: LogLevel) => Promise<void>;
}

/** 保存日志等级偏好，并立即同步后端日志过滤器。 */
export const useLogLevelStore = create<LogLevelState>((set) => ({
  logLevel: readLogLevel(),
  async setLogLevel(logLevel) {
    await invoke('set_log_level', { level: logLevel });
    try { localStorage.setItem(LOG_LEVEL_KEY, logLevel); } catch { /* 存储不可用时保留内存偏好。 */ }
    set({ logLevel });
  },
}));
