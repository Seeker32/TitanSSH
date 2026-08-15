import { beforeEach, describe, expect, it, vi } from 'vitest';
import { invoke } from '@tauri-apps/api/core';
import { useLogsStore } from '@/stores/logs';

const mockInvoke = vi.mocked(invoke);

describe('logs store', () => {
  beforeEach(() => {
    useLogsStore.setState(useLogsStore.getInitialState(), true);
    mockInvoke.mockReset();
  });

  it('load 成功写入最近日志行并清除读取错误', async () => {
    mockInvoke.mockResolvedValue(['line-1', 'line-2']);
    await useLogsStore.getState().load();
    expect(mockInvoke).toHaveBeenCalledWith('get_recent_logs');
    expect(useLogsStore.getState().lines).toEqual(['line-1', 'line-2']);
    expect(useLogsStore.getState().loadError).toBeNull();
  });

  it('load 失败写入结构化错误且保留已有行', async () => {
    useLogsStore.setState({ lines: ['old-line'] });
    mockInvoke.mockRejectedValue({ code: 'IoError', detail: 'read failed' });
    await useLogsStore.getState().load();
    expect(useLogsStore.getState().loadError).toEqual({ code: 'IoError', detail: 'read failed' });
    expect(useLogsStore.getState().lines).toEqual(['old-line']);
  });

  it('load 对非数组响应回退为空列表（IPC 边界防御）', async () => {
    mockInvoke.mockResolvedValue(undefined);
    await useLogsStore.getState().load();
    expect(useLogsStore.getState().lines).toEqual([]);
    expect(useLogsStore.getState().loadError).toBeNull();
  });

  it('export 以所选路径调用后端并清除导出错误', async () => {
    useLogsStore.setState({ exportError: { code: 'IoError', detail: 'stale' } });
    mockInvoke.mockResolvedValue(undefined);
    await useLogsStore.getState().export('/tmp/titanssh.log');
    expect(mockInvoke).toHaveBeenCalledWith('export_logs', { path: '/tmp/titanssh.log' });
    expect(useLogsStore.getState().exportError).toBeNull();
  });

  it('export 失败写入结构化错误', async () => {
    mockInvoke.mockRejectedValue({ code: 'IoError', detail: 'copy failed' });
    await useLogsStore.getState().export('/tmp/titanssh.log');
    expect(useLogsStore.getState().exportError).toEqual({ code: 'IoError', detail: 'copy failed' });
  });
});
