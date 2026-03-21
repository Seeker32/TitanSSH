import { describe, it, expect, beforeEach } from 'vitest';
import { mount } from '@vue/test-utils';
import { createPinia, setActivePinia } from 'pinia';
import TransferQueue from '@/components/sftp/TransferQueue.vue';
import { makeTransferTask } from './fixtures';
import type { TransferTask } from '@/types/sftp';

function makeTasks(overrides: Partial<TransferTask>[] = []): Map<string, TransferTask> {
  const map = new Map<string, TransferTask>();
  overrides.forEach((o, i) => {
    const t = makeTransferTask({ task_id: `task-${i}`, ...o });
    map.set(t.task_id, t);
  });
  return map;
}

describe('TransferQueue', () => {
  beforeEach(() => setActivePinia(createPinia()));

  it('渲染 Running 任务进度条', () => {
    const tasks = makeTasks([{ status: 'Running', transferred_bytes: 25600, total_bytes: 51200 }]);
    const wrapper = mount(TransferQueue, { props: { tasks } });
    expect(wrapper.find('[data-testid="progress-bar"]').exists()).toBe(true);
    const fill = wrapper.find('[data-testid="progress-fill"]');
    expect(fill.attributes('style')).toContain('50%');
  });

  it('渲染 Done 任务进度为 100%', () => {
    const tasks = makeTasks([{ status: 'Done', transferred_bytes: 51200, total_bytes: 51200 }]);
    const wrapper = mount(TransferQueue, { props: { tasks } });
    const fill = wrapper.find('[data-testid="progress-fill"]');
    expect(fill.attributes('style')).toContain('100%');
  });

  it('Failed 任务显示 error_message', () => {
    const tasks = makeTasks([{ status: 'Failed', error_message: '网络中断' }]);
    const wrapper = mount(TransferQueue, { props: { tasks } });
    expect(wrapper.text()).toContain('网络中断');
  });

  it('Cancelled 任务显示"已取消"，不显示 error_message', () => {
    const tasks = makeTasks([{ status: 'Cancelled', error_message: null }]);
    const wrapper = mount(TransferQueue, { props: { tasks } });
    expect(wrapper.text()).toContain('已取消');
    expect(wrapper.text()).not.toContain('null');
  });

  it('Pending 任务显示"等待中"', () => {
    const tasks = makeTasks([{ status: 'Pending' }]);
    const wrapper = mount(TransferQueue, { props: { tasks } });
    expect(wrapper.text()).toContain('等待中');
  });

  it('点击取消按钮触发 cancel 事件', async () => {
    const tasks = makeTasks([{ status: 'Running', task_id: 'task-0' }]);
    const wrapper = mount(TransferQueue, { props: { tasks } });
    await wrapper.find('[data-testid="cancel-btn"]').trigger('click');
    expect(wrapper.emitted('cancel')).toBeTruthy();
    expect(wrapper.emitted('cancel')![0][0]).toBe('task-0');
  });
});
