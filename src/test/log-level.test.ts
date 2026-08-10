import { beforeEach, describe, expect, it, vi } from 'vitest';
import { invoke } from '@tauri-apps/api/core';
import { readLogLevel, useLogLevelStore } from '@/stores/log-level';

const mockInvoke = vi.mocked(invoke);

describe('log level preference', () => {
  beforeEach(() => {
    localStorage.removeItem('log-level');
    useLogLevelStore.setState(useLogLevelStore.getInitialState(), true);
    mockInvoke.mockReset();
  });

  it('falls back to info for missing or invalid persisted levels', () => {
    expect(readLogLevel()).toBe('info');
    localStorage.setItem('log-level', 'verbose');
    expect(readLogLevel()).toBe('info');
  });

  it('persists a level only after the backend accepts it', async () => {
    mockInvoke.mockResolvedValueOnce(undefined);
    await useLogLevelStore.getState().setLogLevel('debug');
    expect(mockInvoke).toHaveBeenCalledWith('set_log_level', { level: 'debug' });
    expect(useLogLevelStore.getState().logLevel).toBe('debug');
    expect(localStorage.getItem('log-level')).toBe('debug');
  });

  it('keeps the current level when the backend rejects a change', async () => {
    mockInvoke.mockRejectedValueOnce(new Error('unavailable'));
    await expect(useLogLevelStore.getState().setLogLevel('trace')).rejects.toThrow('unavailable');
    expect(useLogLevelStore.getState().logLevel).toBe('info');
    expect(localStorage.getItem('log-level')).toBeNull();
  });
});
