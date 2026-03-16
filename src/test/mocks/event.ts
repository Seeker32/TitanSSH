import { vi } from 'vitest';

type EventCallback<T = unknown> = (event: { payload: T }) => void;

const listeners = new Map<string, Set<EventCallback>>();

export const listen = vi.fn(async <T>(eventName: string, callback: EventCallback<T>) => {
  const handlers = listeners.get(eventName) ?? new Set<EventCallback>();
  handlers.add(callback as EventCallback);
  listeners.set(eventName, handlers);

  return () => {
    handlers.delete(callback as EventCallback);
    if (handlers.size === 0) {
      listeners.delete(eventName);
    }
  };
});
export const emit = vi.fn();

export function emitMockEvent<T>(eventName: string, payload: T) {
  for (const callback of listeners.get(eventName) ?? []) {
    callback({ payload });
  }
}

export function resetMockEvents() {
  listeners.clear();
  listen.mockClear();
  emit.mockClear();
}
