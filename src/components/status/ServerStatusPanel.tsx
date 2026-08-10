import { Card, Col, Empty, Progress, Row, Statistic, Typography } from 'antd';
import { ChevronDown, ChevronUp } from 'lucide-react';
import type { MonitorSnapshot } from '@/types/monitor';

interface Props {
  snapshot: MonitorSnapshot | null;
  /** 是否折叠为状态点窄条 */
  collapsed: boolean;
  /** 请求切换折叠状态 */
  onToggle: () => void;
}

/** 将监控数值格式化为百分比文本。 */
function formatPercent(value: number | undefined) {
  return typeof value === 'number' ? `${value.toFixed(1)}%` : '--';
}

/** 将字节容量格式化为易读文本。 */
function formatBytes(bytes: number | undefined) {
  if (typeof bytes !== 'number' || bytes <= 0) return '--';
  const units = ['B', 'KB', 'MB', 'GB', 'TB'];
  let value = bytes;
  let index = 0;
  while (value >= 1024 && index < units.length - 1) {
    value /= 1024;
    index += 1;
  }
  return `${value.toFixed(1)} ${units[index]}`;
}

/** 将每秒字节速率格式化为易读文本，未知值与零流量保持可区分。 */
function formatRate(bytesPerSecond: number | null | undefined) {
  if (typeof bytesPerSecond !== 'number' || bytesPerSecond < 0) return '--';
  const units = ['B', 'KB', 'MB', 'GB', 'TB'];
  let value = bytesPerSecond;
  let index = 0;
  while (value >= 1024 && index < units.length - 1) {
    value /= 1024;
    index += 1;
  }
  return `${value.toFixed(index === 0 ? 0 : 1)} ${units[index]}/s`;
}

/** 根据使用率返回绿、黄、红三级进度色。 */
function progressColor(value: number) {
  if (value < 60) return '#10b981';
  if (value < 85) return '#f59e0b';
  return '#ef4444';
}

/** 渲染后端单次推送的服务器监控快照；折叠态只显示状态点窄条。 */
export default function ServerStatusPanel({ snapshot, collapsed, onToggle }: Props) {
  if (collapsed) {
    return (
      <div className="monitor-strip" data-testid="monitor-strip" role="button" aria-expanded="false" onClick={onToggle}>
        <span className={`status-dot ${snapshot ? 'dot-connected' : 'dot-offline'}`} />
        <span className="monitor-strip-label">监控</span>
        <ChevronDown size={12} className="monitor-strip-chevron" />
      </div>
    );
  }

  const metrics = [
    ['CPU', snapshot?.cpuUsage],
    ['Memory', snapshot?.memoryUsage],
    ['Disk', snapshot?.diskUsage],
  ] as const;
  const defaultInterface = snapshot?.network.available ? snapshot.network.interfaces[0] : undefined;
  return (
    <Card size="small" variant="borderless" className="status-panel"
      title={<><Typography.Text type="secondary">服务器状态</Typography.Text><strong>{snapshot ? '已连接' : '未连接'}</strong></>}
      extra={<button type="button" className="monitor-collapse-btn" data-testid="monitor-collapse-btn"
        aria-label="折叠监视条" title="折叠监视条" onClick={onToggle}><ChevronUp size={12} /></button>}>
      <Row gutter={[12, 12]}>
        {metrics.map(([label, value], index) => (
          <Col span={index === 2 ? 24 : 12} key={label}>
            <Statistic title={label} value={formatPercent(value)} />
            <Progress percent={value ?? 0} strokeColor={snapshot ? progressColor(value ?? 0) : undefined} showInfo={false} />
            {label === 'Disk' && <Typography.Text type="secondary" className="capacity">
              剩余 {formatBytes(snapshot?.diskAvailableBytes)} / 总量 {formatBytes(snapshot?.diskTotalBytes)}
            </Typography.Text>}
          </Col>
        ))}
        {snapshot && (!snapshot.network.available ? (
          <Col span={24}><Typography.Text type="secondary">网络数据不可用</Typography.Text></Col>
        ) : !defaultInterface ? (
          <Col span={24}><Typography.Text type="secondary">无可用网卡</Typography.Text></Col>
        ) : <>
          <Col span={12}><Statistic title={`下行 · ${defaultInterface.name}`} value={formatRate(defaultInterface.receiveBytesPerSecond)} /></Col>
          <Col span={12}><Statistic title={`上行 · ${defaultInterface.name}`} value={formatRate(defaultInterface.transmitBytesPerSecond)} /></Col>
        </>)}
        <Col span={24}><Typography.Text type="secondary" className="updated">
          Updated: {snapshot ? new Date(snapshot.timestamp).toLocaleTimeString() : '--'}
        </Typography.Text></Col>
        {!snapshot && <Col span={24}><Empty description="连接建立后，这里会每 2 秒刷新一次服务器状态" /></Col>}
      </Row>
    </Card>
  );
}
