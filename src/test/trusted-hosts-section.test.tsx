import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import TrustedHostsSection, { endpointLabel } from '@/components/settings/TrustedHostsSection';
import { useLocaleStore } from '@/stores/locale';
import { useTrustedHostsStore } from '@/stores/trusted-hosts';

describe('endpointLabel', () => {
  it('普通主机显示 host:port，IPv6 用标准 [host]:port 无歧义展示', () => {
    expect(endpointLabel('10.0.0.8', 22)).toBe('10.0.0.8:22');
    expect(endpointLabel('prod.example.com', 2222)).toBe('prod.example.com:2222');
    expect(endpointLabel('::1', 22)).toBe('[::1]:22');
    expect(endpointLabel('2001:db8::1', 2200)).toBe('[2001:db8::1]:2200');
  });
});

describe('TrustedHostsSection', () => {
  beforeEach(() => {
    useLocaleStore.setState({ locale: 'zh-CN' });
    useTrustedHostsStore.setState({ status: 'idle', hosts: [], error: null });
  });

  it('加载中展示读取状态', () => {
    useTrustedHostsStore.setState({ status: 'loading', hosts: [], error: null });
    render(<TrustedHostsSection onRetry={vi.fn()} />);
    expect(screen.getByTestId('trusted-hosts-loading')).toHaveTextContent('正在读取信任记录');
  });

  it('渲染只读清单：endpoint、算法与指纹，且无编辑/删除/导入/导出操作', () => {
    useTrustedHostsStore.setState({
      status: 'ready',
      hosts: [
        { host: '10.0.0.8', port: 22, algorithm: 'ssh-ed25519', fingerprint: 'SHA256:aaa' },
        { host: '10.0.0.9', port: 2222, algorithm: 'ssh-rsa', fingerprint: 'SHA256:bbb' },
        { host: '2001:db8::1', port: 22, algorithm: 'ssh-ed25519', fingerprint: 'SHA256:ccc' },
      ],
      error: null,
    });
    render(<TrustedHostsSection onRetry={vi.fn()} />);
    const row = screen.getByTestId('trusted-host-row-10.0.0.8-22');
    expect(row).toHaveTextContent('10.0.0.8:22');
    expect(row).toHaveTextContent('ssh-ed25519');
    expect(row).toHaveTextContent('SHA256:aaa');
    expect(screen.getByTestId('trusted-host-row-10.0.0.9-2222')).toHaveTextContent('10.0.0.9:2222');
    // IPv6 endpoint 以 [host]:port 无歧义展示，不破坏精确拼写
    expect(screen.getByTestId('trusted-host-row-2001:db8::1-22')).toHaveTextContent('[2001:db8::1]:22');
    expect(screen.getByText(/只读清单/)).toBeInTheDocument();
    // UI 不提供删除、编辑、导入或导出操作
    expect(screen.queryAllByRole('button')).toHaveLength(0);
  });

  it('空信任存储展示明确空状态，不伪装成错误', () => {
    useTrustedHostsStore.setState({ status: 'ready', hosts: [], error: null });
    render(<TrustedHostsSection onRetry={vi.fn()} />);
    expect(screen.getByTestId('trusted-hosts-empty')).toHaveTextContent('尚无信任记录');
    expect(screen.queryByTestId('trusted-hosts-list')).toBeNull();
    expect(screen.queryByTestId('trusted-hosts-error')).toBeNull();
  });

  it('读取失败展示结构化错误状态，重试交给页面重新加载', async () => {
    const user = userEvent.setup();
    const onRetry = vi.fn();
    useTrustedHostsStore.setState({
      status: 'error',
      hosts: [],
      error: { code: 'TrustStoreError', detail: '解析信任存储失败: 第 3 行' },
    });
    render(<TrustedHostsSection onRetry={onRetry} />);
    expect(screen.getByTestId('trusted-hosts-error')).toHaveTextContent('无法读取信任记录');
    expect(screen.getByTestId('trusted-hosts-error')).toHaveTextContent('解析信任存储失败: 第 3 行');
    // 错误绝不伪装成空列表
    expect(screen.queryByTestId('trusted-hosts-empty')).toBeNull();

    await user.click(screen.getByTestId('trusted-hosts-retry'));
    expect(onRetry).toHaveBeenCalledTimes(1);
  });

  it('英文界面展示对应文案', () => {
    useLocaleStore.setState({ locale: 'en-US' });
    useTrustedHostsStore.setState({ status: 'ready', hosts: [], error: null });
    render(<TrustedHostsSection onRetry={vi.fn()} />);
    expect(screen.getByTestId('trusted-hosts-empty')).toHaveTextContent('No trust records yet');
  });

  it('英文界面读取失败同样展示明确的错误状态', () => {
    useLocaleStore.setState({ locale: 'en-US' });
    useTrustedHostsStore.setState({
      status: 'error',
      hosts: [],
      error: { code: 'TrustStoreError', detail: 'failed to parse known_hosts' },
    });
    render(<TrustedHostsSection onRetry={vi.fn()} />);
    expect(screen.getByTestId('trusted-hosts-error')).toHaveTextContent('Unable to read trust records');
    expect(screen.getByTestId('trusted-hosts-error')).toHaveTextContent('failed to parse known_hosts');
    expect(screen.queryByTestId('trusted-hosts-empty')).toBeNull();
  });
});
