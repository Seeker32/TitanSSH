import { Card, Col, Empty, Progress, Row, Statistic, Typography } from 'antd';
import { ChevronDown, ChevronUp } from 'lucide-react';
import type { MonitorSnapshot, NetworkTrendSample } from '@/types/monitor';
import type { ProcessSortMode } from '@/stores/process';
import type { ProcessSnapshot } from '@/types/process';
import { translate } from '@/i18n';
import { useLocaleStore } from '@/stores/locale';
import ProcessSummaryPanel from './ProcessSummaryPanel';

interface Props {
  snapshot: MonitorSnapshot | null;
  /** 当前 Session 选择的网卡接口名称。 */
  selectedInterfaceName?: string | null;
  /** 请求切换当前 Session 的网卡接口。 */
  onInterfaceChange?: (interfaceName: string) => void;
  /** 当前 Session 已选网卡的最近一分钟趋势。 */
  trendSamples?: NetworkTrendSample[];
  /** 当前 Session 最新进程快照与摘要排序档位。 */
  processSnapshot: ProcessSnapshot | null;
  processSortMode: ProcessSortMode;
  /** 请求切换进程摘要排序档位。 */
  onProcessSortModeChange: (mode: ProcessSortMode) => void;
  /** 是否折叠为状态点窄条 */
  collapsed: boolean;
  /** 请求切换折叠状态 */
  onToggle: () => void;
}

/** 将监控数值格式化为百分比文本；null/undefined 表示未知。 */
function formatPercent(value: number | null | undefined) {
  return typeof value === 'number' ? `${value.toFixed(1)}%` : '--';
}

/** 将字节容量格式化为易读文本；null/undefined 表示未知。 */
function formatBytes(bytes: number | null | undefined) {
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

/** 将单个方向的有效样本连接为 SVG 路径，null 样本保留为断点。 */
function trendPath(samples: NetworkTrendSample[], latestTimestamp: number, maximum: number, key: 'receiveBytesPerSecond' | 'transmitBytesPerSecond') {
  let connected = false;
  return samples.map((sample) => {
    const value = sample[key];
    if (value === null) {
      connected = false;
      return '';
    }
    const x = Math.max(0, Math.min(280, ((sample.timestamp - (latestTimestamp - 60_000)) / 60_000) * 280));
    const y = 84 - (value / maximum) * 76;
    const command = connected ? 'L' : 'M';
    connected = true;
    return `${command}${x} ${y}`;
  }).join(' ');
}

/** 渲染零基准、双方向且不掩盖不可用样本的原生一分钟趋势图。 */
function NetworkTrendChart({ samples, locale }: { samples: NetworkTrendSample[]; locale: ReturnType<typeof useLocaleStore.getState>['locale'] }) {
  const latestTimestamp = samples[samples.length - 1]?.timestamp ?? 0;
  const maximum = Math.max(1, ...samples.flatMap((sample) => [sample.receiveBytesPerSecond, sample.transmitBytesPerSecond]).filter((value): value is number => value !== null));
  return <div className="network-trend">
    <div className="network-trend-legend" aria-label={translate(locale, 'monitor.legend')}><span className="network-trend-down">{translate(locale, 'monitor.downTrend')}</span><span className="network-trend-up">{translate(locale, 'monitor.upTrend')}</span></div>
    <svg role="img" aria-label={translate(locale, 'monitor.trend')} viewBox="0 0 280 96" preserveAspectRatio="none">
      <line x1="0" y1="84" x2="280" y2="84" stroke="currentColor" opacity="0.3" />
      <path d={trendPath(samples, latestTimestamp, maximum, 'receiveBytesPerSecond')} stroke="#38bdf8" strokeWidth="2" fill="none" />
      <path d={trendPath(samples, latestTimestamp, maximum, 'transmitBytesPerSecond')} stroke="#f59e0b" strokeWidth="2" fill="none" />
    </svg>
    <div className="network-trend-boundary" aria-hidden="true"><span>{translate(locale, 'monitor.ago')}</span><span>{translate(locale, 'monitor.now')}</span></div>
  </div>;
}

/** 渲染后端单次推送的服务器监控快照；折叠态只显示状态点窄条。 */
export default function ServerStatusPanel({ snapshot, selectedInterfaceName, onInterfaceChange, trendSamples = [], processSnapshot, processSortMode, onProcessSortModeChange, collapsed, onToggle }: Props) {
  const locale = useLocaleStore((state) => state.locale);
  if (collapsed) {
    return (
      <div className="monitor-strip" data-testid="monitor-strip" role="button" aria-expanded="false" onClick={onToggle}>
        <span className={`status-dot ${snapshot ? 'dot-connected' : 'dot-offline'}`} />
        <span className="monitor-strip-label">{translate(locale, 'monitor.name')}</span>
        <ChevronUp size={12} className="monitor-strip-chevron" />
      </div>
    );
  }

  const metrics = [
    ['CPU', snapshot?.cpuUsage],
    ['Memory', snapshot?.memoryUsage],
    ['Disk', snapshot?.diskUsage],
  ] as const;
  const selectedInterface = snapshot?.network.available
    ? snapshot.network.interfaces.find((item) => item.name === selectedInterfaceName) ?? snapshot.network.interfaces[0]
    : undefined;
  return (
    <Card size="small" variant="borderless" className="status-panel"
      title={<><Typography.Text type="secondary">{translate(locale, 'monitor.title')}</Typography.Text><strong>{snapshot ? translate(locale, 'monitor.connected') : translate(locale, 'monitor.disconnected')}</strong></>}
      extra={<button type="button" className="monitor-collapse-btn" data-testid="monitor-collapse-btn"
        aria-label={translate(locale, 'monitor.collapse')} title={translate(locale, 'monitor.collapse')} onClick={onToggle}><ChevronDown size={12} /></button>}>
      <Row gutter={[12, 12]}>
        {metrics.map(([label, value], index) => (
          <Col span={index === 2 ? 24 : 12} key={label}>
            <Statistic title={label} value={formatPercent(value)} />
            <Progress percent={value ?? 0} strokeColor={snapshot ? progressColor(value ?? 0) : undefined} showInfo={false} />
            {label === 'Memory' && <Typography.Text type="secondary" className="capacity">
              {translate(locale, 'monitor.memoryCapacity', { used: formatBytes(snapshot?.memoryUsedBytes), total: formatBytes(snapshot?.memoryTotalBytes) })}
            </Typography.Text>}
            {label === 'Disk' && <Typography.Text type="secondary" className="capacity">
              {translate(locale, 'monitor.capacity', { available: formatBytes(snapshot?.diskAvailableBytes), total: formatBytes(snapshot?.diskTotalBytes) })}
            </Typography.Text>}
          </Col>
        ))}
        {snapshot && (!snapshot.network.available ? (
          <Col span={24}><Typography.Text type="secondary">{translate(locale, 'monitor.unavailable')}</Typography.Text></Col>
        ) : !selectedInterface ? (
          <Col span={24}><Typography.Text type="secondary">{translate(locale, 'monitor.noInterface')}</Typography.Text></Col>
        ) : <>
          <Col span={24}><label>{translate(locale, 'monitor.interface')} <select aria-label={translate(locale, 'monitor.interface')} value={selectedInterface.name}
            onChange={(event) => onInterfaceChange?.(event.target.value)}>{snapshot.network.interfaces.map((item) => (
              <option key={item.name} value={item.name}>{item.name}</option>
            ))}</select></label></Col>
          <Col span={24}><NetworkTrendChart samples={trendSamples} locale={locale} /></Col>
          <Col span={12}><Statistic title={translate(locale, 'monitor.down', { name: selectedInterface.name })} value={formatRate(selectedInterface.receiveBytesPerSecond)} /></Col>
          <Col span={12}><Statistic title={translate(locale, 'monitor.up', { name: selectedInterface.name })} value={formatRate(selectedInterface.transmitBytesPerSecond)} /></Col>
        </>)}
        {!snapshot && <Col span={24}><Empty description={translate(locale, 'monitor.empty')} /></Col>}
        <Col span={24}><ProcessSummaryPanel snapshot={processSnapshot} sortMode={processSortMode} onSortModeChange={onProcessSortModeChange} /></Col>
      </Row>
    </Card>
  );
}
