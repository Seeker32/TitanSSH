import { describe, it, expect, beforeEach } from 'vitest';
import { mount } from '@vue/test-utils';
import { createPinia, setActivePinia } from 'pinia';
import FileExplorer from '@/components/sftp/FileExplorer.vue';
import { makeRemoteEntry, makeRemoteDir } from './fixtures';
import type { SftpSessionState } from '@/types/sftp';

function makeState(overrides: Partial<SftpSessionState> = {}): SftpSessionState {
  return {
    currentPath: '/var/log',
    entries: [makeRemoteDir(), makeRemoteEntry()],
    selectedPaths: new Set(),
    loading: false,
    error: null,
    tasks: new Map(),
    ...overrides,
  };
}

describe('FileExplorer', () => {
  beforeEach(() => setActivePinia(createPinia()));

  it('渲染面包屑路径', () => {
    const wrapper = mount(FileExplorer, { props: { state: makeState() } });
    expect(wrapper.text()).toContain('var');
    expect(wrapper.text()).toContain('log');
  });

  it('渲染文件列表，目录在前', () => {
    const wrapper = mount(FileExplorer, { props: { state: makeState() } });
    const rows = wrapper.findAll('[data-testid="file-row"]');
    expect(rows[0].text()).toContain('nginx'); // 目录
    expect(rows[1].text()).toContain('syslog'); // 文件
  });

  it('点击文件行触发 select 事件', async () => {
    const wrapper = mount(FileExplorer, { props: { state: makeState() } });
    const fileRow = wrapper.findAll('[data-testid="file-row"]')[1];
    await fileRow.trigger('click');
    expect(wrapper.emitted('select')).toBeTruthy();
    expect(wrapper.emitted('select')![0][0]).toBe('/var/log/syslog');
  });

  it('双击目录触发 navigate 事件', async () => {
    const wrapper = mount(FileExplorer, { props: { state: makeState() } });
    const dirRow = wrapper.findAll('[data-testid="file-row"]')[0];
    await dirRow.trigger('dblclick');
    expect(wrapper.emitted('navigate')).toBeTruthy();
    expect(wrapper.emitted('navigate')![0][0]).toBe('/var/log/nginx');
  });

  it('loading 状态显示加载提示', () => {
    const wrapper = mount(FileExplorer, { props: { state: makeState({ loading: true, entries: [] }) } });
    expect(wrapper.text()).toContain('加载中');
  });

  it('空目录显示空状态提示', () => {
    const wrapper = mount(FileExplorer, { props: { state: makeState({ entries: [] }) } });
    expect(wrapper.text()).toContain('空目录');
  });

  it('error 状态显示错误信息', () => {
    const wrapper = mount(FileExplorer, {
      props: { state: makeState({ error: 'SFTP 通道错误', entries: [] }) },
    });
    expect(wrapper.text()).toContain('SFTP 通道错误');
  });
});
