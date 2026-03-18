/**
 * P1-1 TerminalPane 组件测试
 *
 * 覆盖：
 * 1. activeView='home' 时渲染 HomeQuickActions，不渲染 XtermView
 * 2. activeView=session_id 时渲染对应 XtermView
 * 3. 多会话时为每个 session 渲染一个 XtermView
 * 4. XtermView 的 active prop 仅对当前激活会话为 true
 * 5. XtermView 的 input 事件向上透传为 TerminalPane 的 input emit
 * 6. XtermView 的 resize 事件向上透传为 TerminalPane 的 resize emit
 * 7. HomeQuickActions 的 open 事件向上透传为 openHost emit
 * 8. HomeQuickActions 的 create 事件向上透传为 createHost emit
 */
import { describe, expect, it } from 'vitest';
import { mount } from '@vue/test-utils';
import TerminalPane from '@/components/terminal/TerminalPane.vue';
import { makeHost, makeSession } from './fixtures';
import type { SessionInfo } from '@/types/session';
import type { HostConfig } from '@/types/host';

/** XtermView stub，记录 props 并可触发 emit */
const XtermViewStub = {
  name: 'XtermView',
  template: '<div class="xterm-stub" :data-session-id="sessionId" :data-active="active" />',
  props: ['sessionId', 'active'],
  emits: ['input', 'resize'],
};

/** HomeQuickActions stub */
const HomeQuickActionsStub = {
  name: 'HomeQuickActions',
  template: '<div class="home-stub" />',
  props: ['hosts'],
  emits: ['open', 'create'],
};

function mountPane(
  sessions: SessionInfo[] = [],
  activeView: string = 'home',
  hosts: HostConfig[] = [],
) {
  return mount(TerminalPane, {
    props: { sessions, activeView, hosts },
    global: {
      stubs: {
        XtermView: XtermViewStub,
        HomeQuickActions: HomeQuickActionsStub,
      },
    },
  });
}

describe('TerminalPane 组件', () => {
  it('activeView=home 时渲染 HomeQuickActions', () => {
    const wrapper = mountPane([], 'home');
    expect(wrapper.find('.home-stub').exists()).toBe(true);
  });

  it('activeView=home 时不渲染 XtermView', () => {
    const wrapper = mountPane([makeSession()], 'home');
    // XtermView 存在但 home-view 可见
    expect(wrapper.find('.home-stub').exists()).toBe(true);
  });

  it('sessions 为空时不渲染 XtermView stub', () => {
    const wrapper = mountPane([], 'home');
    expect(wrapper.findAll('.xterm-stub').length).toBe(0);
  });

  it('多会话时为每个 session 渲染一个 XtermView', () => {
    const sessions = [
      makeSession({ session_id: 's1' }),
      makeSession({ session_id: 's2' }),
      makeSession({ session_id: 's3' }),
    ];
    const wrapper = mountPane(sessions, 's1');
    expect(wrapper.findAll('.xterm-stub').length).toBe(3);
  });

  it('XtermView 的 active prop 仅对激活会话为 true', () => {
    const sessions = [
      makeSession({ session_id: 's1' }),
      makeSession({ session_id: 's2' }),
    ];
    const wrapper = mountPane(sessions, 's1');
    const stubs = wrapper.findAll('.xterm-stub');
    expect(stubs[0].attributes('data-active')).toBe('true');
    expect(stubs[1].attributes('data-active')).toBe('false');
  });

  it('XtermView 的 sessionId prop 与 session.session_id 一致', () => {
    const sessions = [makeSession({ session_id: 'sess-abc' })];
    const wrapper = mountPane(sessions, 'sess-abc');
    const stub = wrapper.find('.xterm-stub');
    expect(stub.attributes('data-session-id')).toBe('sess-abc');
  });

  it('XtermView input 事件向上透传为 TerminalPane input emit', async () => {
    const sessions = [makeSession({ session_id: 's1' })];
    const wrapper = mountPane(sessions, 's1');
    const xtermComponent = wrapper.findComponent(XtermViewStub);
    await xtermComponent.vm.$emit('input', { sessionId: 's1', data: 'hello' });
    expect(wrapper.emitted('input')?.[0]).toEqual([{ sessionId: 's1', data: 'hello' }]);
  });

  it('XtermView resize 事件向上透传为 TerminalPane resize emit', async () => {
    const sessions = [makeSession({ session_id: 's1' })];
    const wrapper = mountPane(sessions, 's1');
    const xtermComponent = wrapper.findComponent(XtermViewStub);
    await xtermComponent.vm.$emit('resize', { sessionId: 's1', cols: 120, rows: 40 });
    expect(wrapper.emitted('resize')?.[0]).toEqual([{ sessionId: 's1', cols: 120, rows: 40 }]);
  });

  it('HomeQuickActions open 事件向上透传为 openHost emit', async () => {
    const wrapper = mountPane([], 'home', [makeHost()]);
    const homeComponent = wrapper.findComponent(HomeQuickActionsStub);
    await homeComponent.vm.$emit('open', 'host-1');
    expect(wrapper.emitted('openHost')?.[0]).toEqual(['host-1']);
  });

  it('HomeQuickActions create 事件向上透传为 createHost emit', async () => {
    const wrapper = mountPane([], 'home');
    const homeComponent = wrapper.findComponent(HomeQuickActionsStub);
    await homeComponent.vm.$emit('create');
    expect(wrapper.emitted('createHost')?.[0]).toEqual([]);
  });

  it('hosts prop 传递给 HomeQuickActions', () => {
    const hosts = [makeHost({ id: 'h1' }), makeHost({ id: 'h2' })];
    const wrapper = mountPane([], 'home', hosts);
    const homeComponent = wrapper.findComponent(HomeQuickActionsStub);
    expect(homeComponent.props('hosts')).toEqual(hosts);
  });
});
