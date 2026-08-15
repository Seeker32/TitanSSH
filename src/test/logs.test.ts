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

  it('load 对非数组 payload 视为读取失败：保留旧行并写入错误', async () => {
    useLogsStore.setState({ lines: ['old-line'] });
    mockInvoke.mockResolvedValue(undefined);
    await useLogsStore.getState().load();
    expect(useLogsStore.getState().lines).toEqual(['old-line']);
    expect(useLogsStore.getState().loadError).toEqual({
      code: 'Unknown',
      detail: 'Invalid log payload (get_recent_logs)',
    });
  });

  it('exportLogs 调用后端导出并清除导出错误', async () => {
    useLogsStore.setState({ exportError: { code: 'IoError', detail: 'stale' } });
    mockInvoke.mockResolvedValue(undefined);
    await useLogsStore.getState().exportLogs();
    expect(mockInvoke).toHaveBeenCalledWith('export_logs');
    expect(useLogsStore.getState().exportError).toBeNull();
  });

  it('exportLogs 失败写入结构化错误', async () => {
    mockInvoke.mockRejectedValue({ code: 'IoError', detail: 'copy failed' });
    await useLogsStore.getState().exportLogs();
    expect(useLogsStore.getState().exportError).toEqual({ code: 'IoError', detail: 'copy failed' });
  });

  it('慢请求晚到不覆盖新响应（latest-wins）', async () => {
    let resolveSlow!: (value: unknown) => void;
    let resolveFast!: (value: unknown) => void;
    mockInvoke
      .mockImplementationOnce(() => new Promise((resolve) => { resolveSlow = resolve; }))
      .mockImplementationOnce(() => new Promise((resolve) => { resolveFast = resolve; }));
    const slow = useLogsStore.getState().load();
    const fast = useLogsStore.getState().load();
    // 后发起的请求先返回
    resolveFast(['fresh-line']);
    await fast;
    expect(useLogsStore.getState().lines).toEqual(['fresh-line']);
    // 先发起的慢请求后返回：不得覆盖新数据
    resolveSlow(['stale-line']);
    await slow;
    expect(useLogsStore.getState().lines).toEqual(['fresh-line']);
    expect(useLogsStore.getState().loadError).toBeNull();
  });

  it('慢请求晚到的失败同样不覆盖新状态', async () => {
    let rejectSlow!: (error: unknown) => void;
    let resolveFast!: (value: unknown) => void;
    mockInvoke
      .mockImplementationOnce(() => new Promise((_, reject) => { rejectSlow = reject; }))
      .mockImplementationOnce(() => new Promise((resolve) => { resolveFast = resolve; }));
    const slow = useLogsStore.getState().load();
    const fast = useLogsStore.getState().load();
    resolveFast(['fresh-line']);
    await fast;
    rejectSlow({ code: 'IoError', detail: 'stale fail' });
    await slow;
    expect(useLogsStore.getState().lines).toEqual(['fresh-line']);
    expect(useLogsStore.getState().loadError).toBeNull();
  });
});
