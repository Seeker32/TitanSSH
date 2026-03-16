import { beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import { useHostStore } from '@/stores/host';
import { invoke } from '@tauri-apps/api/core';
import { makeHost } from './fixtures';

describe('host store', () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    vi.mocked(invoke).mockReset();
  });

  it('loads hosts into state', async () => {
    vi.mocked(invoke).mockResolvedValueOnce([makeHost(), makeHost({ id: 'host-2' })]);
    const store = useHostStore();

    await store.loadHosts();

    expect(invoke).toHaveBeenCalledWith('list_hosts');
    expect(store.hosts).toHaveLength(2);
    expect(store.error).toBeNull();
  });

  it('keeps error when loading fails', async () => {
    vi.mocked(invoke).mockRejectedValueOnce(new Error('boom'));
    const store = useHostStore();

    await store.loadHosts();

    expect(store.hosts).toEqual([]);
    expect(store.error).toContain('boom');
    expect(store.loading).toBe(false);
  });

  it('saves and deletes hosts through tauri commands', async () => {
    const savedHosts = [makeHost()];
    const deletedHosts = [makeHost({ id: 'host-2' })];
    vi.mocked(invoke)
      .mockResolvedValueOnce(savedHosts)
      .mockResolvedValueOnce(deletedHosts);
    const store = useHostStore();

    await store.saveHost(makeHost());
    await store.deleteHost('host-1');

    expect(invoke).toHaveBeenNthCalledWith(1, 'save_host', {
      hostConfig: makeHost(),
    });
    expect(invoke).toHaveBeenNthCalledWith(2, 'delete_host', { hostId: 'host-1' });
    expect(store.hosts).toEqual(deletedHosts);
  });
});
