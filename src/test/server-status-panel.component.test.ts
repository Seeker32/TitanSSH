import { describe, it, expect } from 'vitest';
import { mount } from '@vue/test-utils';
import ServerStatusPanel from '@/components/status/ServerStatusPanel.vue';
import type { MonitorSnapshot } from '@/types/monitor';

describe('ServerStatusPanel', () => {
  it('renders metric placeholders when snapshot is null', () => {
    const wrapper = mount(ServerStatusPanel, {
      props: { snapshot: null },
    });

    const progressBars = wrapper.findAll('.n-progress');

    expect(wrapper.text()).toContain('未连接');
    expect(wrapper.text()).toContain('CPU');
    expect(wrapper.text()).toContain('Memory');
    expect(wrapper.text()).toContain('Disk');
    expect(wrapper.text()).toContain('--');
    expect(progressBars).toHaveLength(3);
  });

  it('renders cpu_usage from MonitorSnapshot', () => {
    const snapshot: MonitorSnapshot = {
      session_id: 'session-1',
      timestamp: 1_710_000_000_000,
      cpu_usage: 42.5,
      memory_usage: 60.0,
      disk_usage: 75.0,
      disk_available_bytes: 128 * 1024 * 1024 * 1024,
      disk_total_bytes: 512 * 1024 * 1024 * 1024,
    };
    const wrapper = mount(ServerStatusPanel, {
      props: { snapshot },
    });
    expect(wrapper.text()).toContain('42.5');
    expect(wrapper.text()).toContain('60.0');
    expect(wrapper.text()).toContain('75.0');
    expect(wrapper.text()).toContain('剩余 128.0 GB / 总量 512.0 GB');
  });

  it('shows warning color when usage exceeds 60%', () => {
    const snapshot: MonitorSnapshot = {
      session_id: 's1', timestamp: 1_710_000_000_000,
      cpu_usage: 70, memory_usage: 70, disk_usage: 70,
      disk_available_bytes: 128,
      disk_total_bytes: 512,
    };
    const wrapper = mount(ServerStatusPanel, { props: { snapshot } });
    // 进度条应有 warning 状态
    expect(wrapper.html()).toContain('warning');
  });

  it('shows error color when usage is >= 85%', () => {
    const snapshot: MonitorSnapshot = {
      session_id: 's1', timestamp: 1_710_000_000_000,
      cpu_usage: 90, memory_usage: 90, disk_usage: 90,
      disk_available_bytes: 128,
      disk_total_bytes: 512,
    };
    const wrapper = mount(ServerStatusPanel, { props: { snapshot } });
    // 进度条应有 error 状态（≥85% 触发红色）
    expect(wrapper.html()).toContain('error');
  });

  it('displays formatted timestamp in Updated field', () => {
    const snapshot: MonitorSnapshot = {
      session_id: 's1',
      timestamp: 1_710_000_000_000,
      cpu_usage: 10, memory_usage: 20, disk_usage: 30,
      disk_available_bytes: 128,
      disk_total_bytes: 512,
    };
    const wrapper = mount(ServerStatusPanel, { props: { snapshot } });
    // Updated 字段应显示格式化后的时间字符串（非 '--'）
    expect(wrapper.text()).toContain('Updated');
    expect(wrapper.text()).not.toContain('--');
  });

  it('shows placeholder updated field when snapshot is null', () => {
    const wrapper = mount(ServerStatusPanel, { props: { snapshot: null } });
    expect(wrapper.text()).toContain('Updated: --');
  });

  it('shows placeholder disk capacity text when snapshot is null', () => {
    const wrapper = mount(ServerStatusPanel, { props: { snapshot: null } });
    expect(wrapper.text()).toContain('剩余 -- / 总量 --');
  });
});
