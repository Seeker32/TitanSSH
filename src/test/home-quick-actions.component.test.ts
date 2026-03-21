import { describe, expect, it } from 'vitest';
import { mount } from '@vue/test-utils';
import HomeQuickActions from '@/components/home/HomeQuickActions.vue';
import { makeHost } from './fixtures';

describe('HomeQuickActions', () => {
  it('emits open when clicking a host card', async () => {
    const wrapper = mount(HomeQuickActions, {
      props: {
        hosts: [makeHost()],
      },
    });

    await wrapper.get('.host-btn').trigger('click');

    expect(wrapper.emitted('open')).toEqual([['host-1']]);
  });

  it('emits edit without opening the host when clicking edit action', async () => {
    const wrapper = mount(HomeQuickActions, {
      props: {
        hosts: [makeHost()],
      },
    });

    await wrapper.get('.host-action-btn--edit').trigger('click');

    expect(wrapper.emitted('edit')).toEqual([['host-1']]);
    expect(wrapper.emitted('open')).toBeUndefined();
  });

  it('emits remove without opening the host when clicking remove action', async () => {
    const wrapper = mount(HomeQuickActions, {
      props: {
        hosts: [makeHost()],
      },
    });

    await wrapper.get('.host-action-btn--remove').trigger('click');

    expect(wrapper.emitted('remove')).toEqual([['host-1']]);
    expect(wrapper.emitted('open')).toBeUndefined();
  });
});
