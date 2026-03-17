/**
 * 为 @tauri-apps/api/event 模块补充测试辅助函数的类型声明。
 * 这些函数由 src/test/mocks/event.ts 实现，通过 vitest.config.ts 的路径别名注入。
 * 此声明文件使 tsc 在类型检查时能识别这些测试专用导出，避免 TS2305 错误。
 *
 * 注意：使用 augmentation 时必须同时重新导出真实模块的公共成员，
 * 否则 tsc 会将此声明视为完整模块定义而丢失原有导出。
 */
declare module '@tauri-apps/api/event' {
  // 重新导出真实模块中使用到的成员，保持类型兼容
  export type EventName = string | `${string}:${string}`;
  export type EventCallback<T> = (event: Event<T>) => void;
  export interface Event<T> {
    event: EventName;
    id: number;
    payload: T;
  }
  export type UnlistenFn = () => void;

  /** 注册事件监听器，返回取消监听的清理函数 */
  export function listen<T>(
    event: EventName,
    handler: EventCallback<T>,
  ): Promise<UnlistenFn>;

  /** 向所有已注册的监听器触发一个模拟事件，用于单元测试 */
  export function emitMockEvent<T>(eventName: string, payload: T): void;
  /** 清空所有已注册的事件监听器，用于测试间隔离 */
  export function resetMockEvents(): void;
}
