import { describe, expect, it, vi } from 'vitest';
import { invoke } from '@tauri-apps/api/core';
import { useTrustedHostsStore } from '@/stores/trusted-hosts';

const mockInvoke = vi.mocked(invoke);

describe('trusted hosts store', () => {
  it('加载成功：status 变为 ready 并持有后端返回的 typed JSON', async () => {
    mockInvoke.mockResolvedValueOnce([
      { host: '10.0.0.8', port: 22, algorithm: 'ssh-ed25519', fingerprint: 'SHA256:aaa' },
    ]);
    const loading = useTrustedHostsStore.getState().load();
    expect(useTrustedHostsStore.getState().status).toBe('loading');
    await loading;
    expect(useTrustedHostsStore.getState().status).toBe('ready');
    expect(useTrustedHostsStore.getState().hosts).toEqual([
      { host: '10.0.0.8', port: 22, algorithm: 'ssh-ed25519', fingerprint: 'SHA256:aaa' },
    ]);
    expect(useTrustedHostsStore.getState().error).toBeNull();
    expect(mockInvoke).toHaveBeenCalledWith('list_trusted_hosts');
  });

  it('读取失败：status 变为 error 并保留结构化错误，不伪装成空列表', async () => {
    mockInvoke.mockRejectedValueOnce({ code: 'TrustStoreError', detail: '解析信任存储失败: 第 3 行' });
    await useTrustedHostsStore.getState().load();
    expect(useTrustedHostsStore.getState().status).toBe('error');
    expect(useTrustedHostsStore.getState().hosts).toEqual([]);
    expect(useTrustedHostsStore.getState().error).toEqual({ code: 'TrustStoreError', detail: '解析信任存储失败: 第 3 行' });
  });

  it('错误后重新加载成功可恢复为 ready', async () => {
    mockInvoke.mockRejectedValueOnce({ code: 'TrustStoreError', detail: 'boom' });
    await useTrustedHostsStore.getState().load();
    expect(useTrustedHostsStore.getState().status).toBe('error');

    mockInvoke.mockResolvedValueOnce([]);
    await useTrustedHostsStore.getState().load();
    expect(useTrustedHostsStore.getState().status).toBe('ready');
    expect(useTrustedHostsStore.getState().hosts).toEqual([]);
    expect(useTrustedHostsStore.getState().error).toBeNull();
  });
});
