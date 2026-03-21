import { describe, it, expect, beforeEach } from 'vitest';
import { mount } from '@vue/test-utils';
import { createPinia, setActivePinia } from 'pinia';
import SftpPanel from '@/components/sftp/SftpPanel.vue';
import { makeRemoteEntry, makeTransferTask } from './fixtures';
import type { SftpSessionState } from '@/types/sftp';

function makeState(overrides: Partial<SftpSessionState> = {}): SftpSessionState {
  return {
    currentPath: '/var/log',
    entries: [makeRemoteEntry()],
    selectedPaths: new Set(),
    loading: false,
    error: null,
    tasks: new Map([['task-1', makeTransferTask({ task_id: 'task-1' })]]),
    ...overrides,
  };
}

describe('SftpPanel', () => {
  beforeEach(() => setActivePinia(createPinia()));

  it('默认渲染文件浏览器视图', () => {
    const wrapper = mount(SftpPanel, { props: { sessionId: 'session-1', state: makeState() } });
    expect(wrapper.find('[data-testid="file-explorer"]').exists()).toBe(true);
    expect(wrapper.find('[data-testid="transfer-queue"]').exists()).toBe(false);
  });

  it('点击"传输队列"tab 切换视图', async () => {
    const wrapper = mount(SftpPanel, { props: { sessionId: 'session-1', state: makeState() } });
    await wrapper.find('[data-testid="tab-queue"]').trigger('click');
    expect(wrapper.find('[data-testid="transfer-queue"]').exists()).toBe(true);
    expect(wrapper.find('[data-testid="file-explorer"]').exists()).toBe(false);
  });

  it('点击"文件浏览器"tab 切换回文件视图', async () => {
    const wrapper = mount(SftpPanel, { props: { sessionId: 'session-1', state: makeState() } });
    await wrapper.find('[data-testid="tab-queue"]').trigger('click');
    await wrapper.find('[data-testid="tab-explorer"]').trigger('click');
    expect(wrapper.find('[data-testid="file-explorer"]').exists()).toBe(true);
  });

  it('渲染 resizer 分割线', () => {
    const wrapper = mount(SftpPanel, { props: { sessionId: 'session-1', state: makeState() } });
    expect(wrapper.find('[data-testid="sftp-resizer"]').exists()).toBe(true);
  });

  it('state 为 null 时显示占位提示', () => {
    const wrapper = mount(SftpPanel, { props: { sessionId: '', state: null } });
    expect(wrapper.text()).toContain('请选择会话');
  });
});
