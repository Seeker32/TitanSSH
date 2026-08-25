import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import ProcessSummaryPanel from '@/components/status/ProcessSummaryPanel';
import type { ProcessSnapshot } from '@/types/process';

const processSnapshot: ProcessSnapshot = {
  sessionId: 'session-1',
  timestamp: 1,
  totalCount: 2,
  processes: [
    { pid: 1, ppid: 0, user: 'root', command: 'shell', commandLine: 'shell', cpuPercent: 10, memoryBytes: 1024, state: 'S' },
    { pid: 2, ppid: 1, user: 'root', command: 'worker', commandLine: 'worker --serve', cpuPercent: 80, memoryBytes: 2048, state: 'R' },
  ],
};

describe('ProcessSummaryPanel', () => {
  it('显示 top-5 进程并允许切换 CPU/内存排序档', () => {
    const onSortModeChange = vi.fn();
    render(<ProcessSummaryPanel snapshot={processSnapshot} sortMode="cpu" onSortModeChange={onSortModeChange} />);

    expect(screen.getByTestId('process-summary')).toHaveTextContent('进程摘要');
    expect(screen.getByText('worker')).toBeInTheDocument();
    expect(screen.getByText('PID 2')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: '内存' }));
    expect(onSortModeChange).toHaveBeenCalledWith('memory');
  });

  it('点击 top-5 面板或可访问按钮都能打开全量进程标签', () => {
    const onOpenProcess = vi.fn();
    render(<ProcessSummaryPanel snapshot={processSnapshot} sortMode="cpu" onSortModeChange={vi.fn()} onOpenProcess={onOpenProcess} />);

    fireEvent.click(screen.getByText('worker'));
    fireEvent.click(screen.getByRole('button', { name: '查看全部进程' }));
    expect(onOpenProcess).toHaveBeenCalledTimes(2);
  });
});
