import { act, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { invoke } from '@tauri-apps/api/core';
import { save } from '@tauri-apps/plugin-dialog';
import LogViewer from '@/components/settings/LogViewer';
import { useLogsStore } from '@/stores/logs';

const mockInvoke = vi.mocked(invoke);
const mockSaveDialog = vi.mocked(save);

describe('LogViewer', () => {
  beforeEach(() => {
    useLogsStore.setState(useLogsStore.getInitialState(), true);
    mockInvoke.mockReset();
    mockSaveDialog.mockReset();
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

  it('导出经保存对话框复制日志文件；取消对话框不 invoke', async () => {
    const user = userEvent.setup();
    mockInvoke.mockResolvedValue([]);
    render(<LogViewer />);
    await screen.findByTestId('log-viewer-empty');
    mockSaveDialog.mockResolvedValueOnce('/tmp/exported.log');
    await user.click(screen.getByTestId('log-export-btn'));
    await waitFor(() => expect(mockInvoke).toHaveBeenCalledWith('export_logs', { path: '/tmp/exported.log' }));
    expect(mockSaveDialog).toHaveBeenCalledWith({
      defaultPath: expect.stringMatching(/^titanssh-\d{4}-\d{2}-\d{2}_\d{2}-\d{2}-\d{2}\.log$/),
    });
    mockSaveDialog.mockResolvedValueOnce(null);
    await user.click(screen.getByTestId('log-export-btn'));
    const exportCalls = mockInvoke.mock.calls.filter(([command]) => command === 'export_logs');
    expect(exportCalls).toHaveLength(1);
  });

  it('导出失败显示结构化错误提示', async () => {
    const user = userEvent.setup();
    mockInvoke.mockResolvedValue([]);
    render(<LogViewer />);
    await screen.findByTestId('log-viewer-empty');
    mockSaveDialog.mockResolvedValueOnce('/tmp/exported.log');
    mockInvoke.mockRejectedValueOnce({ code: 'IoError', detail: 'copy failed' });
    await user.click(screen.getByTestId('log-export-btn'));
    expect(await screen.findByTestId('log-viewer-export-error')).toHaveTextContent(/IO 错误: copy failed/);
  });
});
