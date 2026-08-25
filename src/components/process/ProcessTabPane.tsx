import { Input, Table } from 'antd';
import type { ColumnsType } from 'antd/es/table';
import { useMemo, useState } from 'react';
import { formatMemory } from '@/components/status/ProcessSummaryPanel';
import { translate } from '@/i18n';
import { useLocaleStore } from '@/stores/locale';
import type { ProcessInfo, ProcessSnapshot } from '@/types/process';

interface Props {
  snapshot: ProcessSnapshot | null;
}

/** 将进程表格中的可空数值按稳定顺序比较，未知值排在有效值之后。 */
function compareValues(left: number | string | null, right: number | string | null): number {
  if (left === null && right !== null) return 1;
  if (left !== null && right === null) return -1;
  if (typeof left === 'string' && typeof right === 'string') return left.localeCompare(right);
  return Number(right ?? 0) - Number(left ?? 0);
}

/** 渲染当前会话缓存的全量进程快照；表格排序与虚拟滚动均由 antd 提供。 */
export default function ProcessTabPane({ snapshot }: Props) {
  const locale = useLocaleStore((state) => state.locale);
  const [query, setQuery] = useState('');
  const processes = useMemo(() => {
    const normalized = query.trim().toLocaleLowerCase();
    if (!snapshot || !normalized) return snapshot?.processes ?? [];
    return snapshot.processes.filter((process) => [process.command, process.commandLine, process.user]
      .some((value) => value.toLocaleLowerCase().includes(normalized)));
  }, [query, snapshot]);

  const columns: ColumnsType<ProcessInfo> = [
    { title: translate(locale, 'process.pid'), dataIndex: 'pid', key: 'pid', width: 90, sorter: (a, b) => a.pid - b.pid },
    { title: translate(locale, 'process.ppid'), dataIndex: 'ppid', key: 'ppid', width: 90, sorter: (a, b) => a.ppid - b.ppid },
    { title: translate(locale, 'process.user'), dataIndex: 'user', key: 'user', width: 130, sorter: (a, b) => a.user.localeCompare(b.user) },
    { title: translate(locale, 'process.cpuPercent'), dataIndex: 'cpuPercent', key: 'cpuPercent', width: 100, sorter: (a, b) => compareValues(a.cpuPercent, b.cpuPercent), render: (value: number | null) => value === null ? '--' : `${value.toFixed(1)}%` },
    { title: translate(locale, 'process.memoryBytes'), dataIndex: 'memoryBytes', key: 'memoryBytes', width: 110, sorter: (a, b) => compareValues(a.memoryBytes, b.memoryBytes), render: (value: number | null) => formatMemory(value) },
    { title: translate(locale, 'process.state'), dataIndex: 'state', key: 'state', width: 90, sorter: (a, b) => a.state.localeCompare(b.state) },
    { title: translate(locale, 'process.command'), dataIndex: 'command', key: 'command', width: 180, sorter: (a, b) => a.command.localeCompare(b.command) },
    { title: translate(locale, 'process.commandLine'), dataIndex: 'commandLine', key: 'commandLine', width: 360, sorter: (a, b) => a.commandLine.localeCompare(b.commandLine), render: (value: string) => <span title={value}>{value}</span> },
  ];

  return (
    <section className="process-tab-pane" aria-label={translate(locale, 'process.tab')}>
      <div className="process-tab-toolbar">
        <strong>{translate(locale, 'process.tab')}</strong>
        <span className="process-tab-count">{translate(locale, 'process.count', { count: snapshot?.totalCount ?? 0 })}</span>
        <Input.Search aria-label={translate(locale, 'process.filter')} placeholder={translate(locale, 'process.filter')} allowClear onChange={(event) => setQuery(event.target.value)} />
      </div>
      <Table<ProcessInfo> rowKey="pid" size="small" dataSource={processes} columns={columns} pagination={false} virtual scroll={{ x: 1100, y: 520 }} locale={{ emptyText: translate(locale, 'process.emptyTable') }} />
    </section>
  );
}
