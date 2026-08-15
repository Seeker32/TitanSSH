import { act, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { invoke } from '@tauri-apps/api/core';
import LogViewer from '@/components/settings/LogViewer';
import { useLogsStore } from '@/stores/logs';

const mockInvoke = vi.mocked(invoke);

describe('LogViewer', () => {
  beforeEach(() => {
    useLogsStore.setState(useLogsStore.getInitialState(), true);
    mockInvoke.mockReset();
  });

  it('挂载即加载并渲染日志行（纯文本不解析）', async () => {
    mockInvoke.mockResolvedValue(['2025-06-01 14:30:00.123 [INFO] core: ready']);
    render(<LogViewer />);
    expect(await screen.findByTestId('log-viewer-lines')).toHaveTextContent('core: ready');
  });

  it('无日志时显示空态文案', async () => {
    mockInvoke.mockResolvedValue([]);
    render(<LogViewer />);
    expect(await screen.findByTestId('log-viewer-empty')).toBeInTheDocument();
  });

  it('读取失败显示结构化错误提示', async () => {
    mockInvoke.mockRejectedValue({ code: 'IoError', detail: 'read failed' });
    render(<LogViewer />);
    expect(await screen.findByTestId('log-viewer-load-error')).toHaveTextContent(/IO 错误: read failed/);
  });

  it('刷新按钮重新拉取日志', async () => {
    const user = userEvent.setup();
    mockInvoke.mockResolvedValue([]);
    render(<LogViewer />);
    await screen.findByTestId('log-viewer-empty');
    expect(mockInvoke).toHaveBeenCalledTimes(1);
    await user.click(screen.getByTestId('log-refresh-btn'));
    await waitFor(() => expect(mockInvoke).toHaveBeenCalledTimes(2));
  });

  it('打开期间每 2 秒自动轮询，卸载后停止', async () => {
    vi.useFakeTimers();
    try {
      mockInvoke.mockResolvedValue([]);
      const { unmount } = render(<LogViewer />);
      await act(async () => { await Promise.resolve(); });
      expect(mockInvoke).toHaveBeenCalledTimes(1);
      await act(async () => { vi.advanceTimersByTime(2000); await Promise.resolve(); });
      expect(mockInvoke).toHaveBeenCalledTimes(2);
      unmount();
      await act(async () => { vi.advanceTimersByTime(4000); });
      expect(mockInvoke).toHaveBeenCalledTimes(2);
    } finally {
      vi.useRealTimers();
    }
  });

  it('导出按钮发起后端导出且成功无错误提示', async () => {
    const user = userEvent.setup();
    mockInvoke.mockResolvedValue([]);
    render(<LogViewer />);
    await screen.findByTestId('log-viewer-empty');
    await user.click(screen.getByTestId('log-export-btn'));
    await waitFor(() => expect(mockInvoke).toHaveBeenCalledWith('export_logs'));
    expect(screen.queryByTestId('log-viewer-export-error')).not.toBeInTheDocument();
  });

  it('导出失败显示结构化错误提示', async () => {
    const user = userEvent.setup();
    mockInvoke.mockResolvedValue([]);
    render(<LogViewer />);
    await screen.findByTestId('log-viewer-empty');
    mockInvoke.mockRejectedValueOnce({ code: 'IoError', detail: 'copy failed' });
    await user.click(screen.getByTestId('log-export-btn'));
    expect(await screen.findByTestId('log-viewer-export-error')).toHaveTextContent(/IO 错误: copy failed/);
  });

  it('导出进行中重复点击不重复发起（防重入）', async () => {
    const user = userEvent.setup();
    mockInvoke.mockResolvedValue([]);
    render(<LogViewer />);
    await screen.findByTestId('log-viewer-empty');
    mockInvoke.mockImplementationOnce(() => new Promise(() => {}));
    await user.click(screen.getByTestId('log-export-btn'));
    await user.click(screen.getByTestId('log-export-btn'));
    const exportCalls = mockInvoke.mock.calls.filter(([command]) => command === 'export_logs');
    expect(exportCalls).toHaveLength(1);
  });
});
