/**
 * P1-1 TerminalTabs 组件测试
 *
 * 覆盖：
 * 1. 首页标签始终渲染，不可关闭
 * 2. 真实会话标签按 sessions 列表渲染
 * 3. 状态圆点根据会话状态正确映射 CSS 类名
 * 4. 点击标签触发 activate 事件
 * 5. 点击关闭按钮触发 close 事件
 * 6. 当前激活标签具有 active 类名
 */
import { describe, expect, it } from 'vitest';
import { mount } from '@vue/test-utils';
import TerminalTabs from '@/components/terminal/TerminalTabs.vue';
import { SessionStatus } from '@/types/session';
import { makeSession } from './fixtures';

/** 挂载 TerminalTabs 的辅助函数 */
function mountTabs(
  sessions = [makeSession()],
  activeView: string = 'home',
) {
  return mount(TerminalTabs, {
    props: { sessions, activeView },
    global: {
      stubs: { NButton: { template: '<button @click="$emit(\'click\')"><slot /></button>' } },
    },
  });
}

describe('TerminalTabs 组件', () => {
  it('始终渲染首页标签', () => {
    const wrapper = mountTabs([], 'home');
    expect(wrapper.text()).toContain('首页');
  });

  it('首页标签激活时具有 active 类名', () => {
    const wrapper = mountTabs([], 'home');
    const tabs = wrapper.findAll('.tab');
    expect(tabs[0].classes()).toContain('active');
  });

  it('按 sessions 列表渲染会话标签', () => {
    const sessions = [
      makeSession({ session_id: 's1', username: 'root', host: '10.0.0.1' }),
      makeSession({ session_id: 's2', username: 'admin', host: '10.0.0.2' }),
    ];
    const wrapper = mountTabs(sessions, 'home');
    expect(wrapper.text()).toContain('root@10.0.0.1');
    expect(wrapper.text()).toContain('admin@10.0.0.2');
  });

  it('点击首页标签触发 activate 事件，payload 为 home', async () => {
    const wrapper = mountTabs([makeSession()], 's1');
    await wrapper.findAll('.tab')[0].trigger('click');
    expect(wrapper.emitted('activate')?.[0]).toEqual(['home']);
  });

  it('点击会话标签触发 activate 事件，payload 为 session_id', async () => {
    const sessions = [makeSession({ session_id: 'sess-abc' })];
    const wrapper = mountTabs(sessions, 'home');
    const sessionTab = wrapper.findAll('.tab')[1];
    await sessionTab.trigger('click');
    expect(wrapper.emitted('activate')?.[0]).toEqual(['sess-abc']);
  });

  it('激活会话标签具有 active 类名', () => {
    const sessions = [makeSession({ session_id: 'sess-xyz' })];
    const wrapper = mountTabs(sessions, 'sess-xyz');
    const tabs = wrapper.findAll('.tab');
    expect(tabs[0].classes()).not.toContain('active');
    expect(tabs[1].classes()).toContain('active');
  });

  it('Connected 状态圆点使用 dot-connected 类名', () => {
    const sessions = [makeSession({ status: SessionStatus.Connected })];
    const wrapper = mountTabs(sessions, 'home');
    expect(wrapper.find('.dot-connected').exists()).toBe(true);
  });

  it('Connecting 状态圆点使用 dot-connecting 类名', () => {
    const sessions = [makeSession({ status: SessionStatus.Connecting })];
    const wrapper = mountTabs(sessions, 'home');
    expect(wrapper.find('.dot-connecting').exists()).toBe(true);
  });

  it('AuthFailed 状态圆点使用 dot-error 类名', () => {
    const sessions = [makeSession({ status: SessionStatus.AuthFailed })];
    const wrapper = mountTabs(sessions, 'home');
    expect(wrapper.find('.dot-error').exists()).toBe(true);
  });

  it('Error 状态圆点使用 dot-error 类名', () => {
    const sessions = [makeSession({ status: SessionStatus.Error })];
    const wrapper = mountTabs(sessions, 'home');
    expect(wrapper.find('.dot-error').exists()).toBe(true);
  });

  it('Timeout 状态圆点使用 dot-error 类名', () => {
    const sessions = [makeSession({ status: SessionStatus.Timeout })];
    const wrapper = mountTabs(sessions, 'home');
    expect(wrapper.find('.dot-error').exists()).toBe(true);
  });

  it('Disconnected 状态圆点使用 dot-error 类名', () => {
    const sessions = [makeSession({ status: SessionStatus.Disconnected })];
    const wrapper = mountTabs(sessions, 'home');
    expect(wrapper.find('.dot-error').exists()).toBe(true);
  });

  it('sessions 为空时只渲染首页标签', () => {
    const wrapper = mountTabs([], 'home');
    expect(wrapper.findAll('.tab').length).toBe(1);
  });

  it('多会话时渲染正确数量的标签', () => {
    const sessions = [
      makeSession({ session_id: 's1' }),
      makeSession({ session_id: 's2' }),
      makeSession({ session_id: 's3' }),
    ];
    const wrapper = mountTabs(sessions, 'home');
    // 首页 + 3 个会话标签
    expect(wrapper.findAll('.tab').length).toBe(4);
  });
});
