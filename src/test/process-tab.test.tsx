import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';
import ProcessTabPane from '@/components/process/ProcessTabPane';
import type { ProcessSnapshot } from '@/types/process';

const processSnapshot: ProcessSnapshot = {
  sessionId: 'session-1',
  timestamp: 1,
  totalCount: 2,
  processes: [
    { pid: 1, ppid: 0, user: 'root', command: 'shell', commandLine: 'shell --login', cpuPercent: 10, memoryBytes: 1024, state: 'S' },
    { pid: 2, ppid: 1, user: 'deploy', command: 'worker', commandLine: 'worker --serve --port 8080', cpuPercent: 80, memoryBytes: 2048, state: 'R' },
  ],
};

describe('ProcessTabPane', () => {
  it('显示全量进程列与完整命令行，并按关键字过滤命令、参数和用户', async () => {
    const user = userEvent.setup();
    render(<ProcessTabPane snapshot={processSnapshot} />);

    expect(screen.getByRole('table')).toBeInTheDocument();
    for (const label of ['PID', 'PPID', '用户', 'CPU%', '内存', '状态', '命令名', '完整命令行']) {
      expect(screen.getByRole('columnheader', { name: label })).toBeInTheDocument();
    }
    expect(screen.getByText('worker --serve --port 8080')).toBeInTheDocument();

    const search = screen.getByRole('searchbox', { name: '过滤进程' });
    await user.type(search, 'deploy');
    expect(screen.getByText('worker')).toBeInTheDocument();
    expect(screen.queryByText('shell')).not.toBeInTheDocument();
  });

  it('支持数值列排序，缺少快照时显示空态', async () => {
    const user = userEvent.setup();
    const { rerender } = render(<ProcessTabPane snapshot={processSnapshot} />);
    const cpu = screen.getByRole('columnheader', { name: 'CPU%' });
    await user.click(cpu);
    await user.click(cpu);
    expect(cpu).toHaveAttribute('aria-sort', 'descending');

    rerender(<ProcessTabPane snapshot={null} />);
    expect(screen.getByText('暂无匹配的进程')).toBeInTheDocument();
  });
});
