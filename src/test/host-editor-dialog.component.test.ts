/**
 * P1-1 HostEditorDialog 组件测试
 *
 * 覆盖：
 * 1. 新建模式：computed title 为"新建连接"
 * 2. 编辑模式：computed title 为"编辑连接"
 * 3. 编辑模式：非敏感字段从 editingHost 初始化到 form reactive 对象
 * 4. 编辑模式：密码/口令字段留空（不预填旧值）
 * 5. 认证方式切换：Password 模式 submit 时 private_key_path/passphrase 为 undefined
 * 6. PrivateKey 模式 submit 时 password 为 undefined
 * 7. 新建时 submit：id 为非空字符串（UUID）
 * 8. 编辑时 submit：id 保持原值
 * 9. close() 触发 update:modelValue false
 * 10. submit() 触发 save 事件，payload 为 SaveHostRequest 形状
 *
 * 策略：直接测试组件实例的内部 form 状态和 emit 行为，
 * 绕过 Naive UI teleport/modal 渲染问题。
 */
import { describe, expect, it } from 'vitest';
import { mount } from '@vue/test-utils';
import { nextTick } from 'vue';
import HostEditorDialog from '@/components/host/HostEditorDialog.vue';
import { AuthType, type HostConfig } from '@/types/host';
import { makeHost } from './fixtures';

/**
 * 挂载 HostEditorDialog，使用 shallow stub 绕过 Naive UI 渲染。
 * 通过 vm 实例直接访问内部状态和方法。
 */
function mountDialog(modelValue = true, editingHost: HostConfig | null = null) {
  return mount(HostEditorDialog, {
    props: { modelValue, editingHost },
    global: {
      // shallow stub 所有 Naive UI 组件，避免 teleport/canvas 问题
      stubs: {
        NModal: true,
        NCard: true,
        NForm: true,
        NFormItem: true,
        NInput: true,
        NInputNumber: true,
        NSelect: true,
        NButton: true,
        NSpace: true,
        NGrid: true,
        NGridItem: true,
      },
    },
  });
}

describe('HostEditorDialog 组件', () => {
  it('新建模式：title computed 为"新建连接"', () => {
    const wrapper = mountDialog(true, null);
    // 直接访问 computed title
    expect((wrapper.vm as any).title).toBe('新建连接');
  });

  it('编辑模式：title computed 为"编辑连接"', () => {
    const wrapper = mountDialog(true, makeHost());
    expect((wrapper.vm as any).title).toBe('编辑连接');
  });

  it('编辑模式：form.name 从 editingHost 初始化', async () => {
    const host = makeHost({ name: 'my-server' });
    const wrapper = mountDialog(true, host);
    await nextTick();
    expect((wrapper.vm as any).form.name).toBe('my-server');
  });

  it('编辑模式：form.host 从 editingHost 初始化', async () => {
    const host = makeHost({ host: '192.168.1.100' });
    const wrapper = mountDialog(true, host);
    await nextTick();
    expect((wrapper.vm as any).form.host).toBe('192.168.1.100');
  });

  it('编辑模式：form.username 从 editingHost 初始化', async () => {
    const host = makeHost({ username: 'deploy' });
    const wrapper = mountDialog(true, host);
    await nextTick();
    expect((wrapper.vm as any).form.username).toBe('deploy');
  });

  it('编辑模式：form.port 从 editingHost 初始化', async () => {
    const host = makeHost({ port: 2222 });
    const wrapper = mountDialog(true, host);
    await nextTick();
    expect((wrapper.vm as any).form.port).toBe(2222);
  });

  it('编辑模式：密码字段留空（不预填旧值）', async () => {
    const host = makeHost({ auth_type: AuthType.Password, password_ref: 'titanssh:host-1:password' });
    const wrapper = mountDialog(true, host);
    await nextTick();
    expect((wrapper.vm as any).form.password).toBe('');
  });

  it('编辑模式：口令字段留空（不预填旧值）', async () => {
    const host = makeHost({ auth_type: AuthType.PrivateKey, passphrase_ref: 'titanssh:host-1:passphrase' });
    const wrapper = mountDialog(true, host);
    await nextTick();
    expect((wrapper.vm as any).form.passphrase).toBe('');
  });

  it('close() 触发 update:modelValue false', async () => {
    const wrapper = mountDialog(true, null);
    await (wrapper.vm as any).close();
    expect(wrapper.emitted('update:modelValue')?.[0]).toEqual([false]);
  });

  it('新建时 submit()：emit save，id 为非空字符串', async () => {
    const wrapper = mountDialog(true, null);
    await nextTick();
    // 填写必填字段
    const vm = wrapper.vm as any;
    vm.form.name = 'test-server';
    vm.form.host = '10.0.0.1';
    vm.form.username = 'root';
    vm.form.auth_type = AuthType.Password;
    vm.form.password = 'secret';
    await nextTick();

    await vm.submit();

    const saved = wrapper.emitted('save');
    expect(saved).toBeTruthy();
    const payload = saved![0][0] as any;
    expect(payload.id).toBeTruthy();
    expect(typeof payload.id).toBe('string');
    expect(payload.name).toBe('test-server');
  });

  it('编辑时 submit()：id 保持原值', async () => {
    const host = makeHost({ id: 'host-original-id' });
    const wrapper = mountDialog(true, host);
    await nextTick();

    await (wrapper.vm as any).submit();

    const payload = wrapper.emitted('save')![0][0] as any;
    expect(payload.id).toBe('host-original-id');
  });

  it('Password 模式 submit()：private_key_path 和 passphrase 为 undefined', async () => {
    const host = makeHost({ auth_type: AuthType.Password });
    const wrapper = mountDialog(true, host);
    await nextTick();

    await (wrapper.vm as any).submit();

    const payload = wrapper.emitted('save')![0][0] as any;
    expect(payload.private_key_path).toBeUndefined();
    expect(payload.passphrase).toBeUndefined();
  });

  it('PrivateKey 模式 submit()：password 为 undefined', async () => {
    const host = makeHost({ auth_type: AuthType.PrivateKey, private_key_path: '~/.ssh/id_rsa' });
    const wrapper = mountDialog(true, host);
    await nextTick();

    await (wrapper.vm as any).submit();

    const payload = wrapper.emitted('save')![0][0] as any;
    expect(payload.password).toBeUndefined();
  });

  it('新建时 submit()：auth_type 正确传递', async () => {
    const wrapper = mountDialog(true, null);
    await nextTick();
    const vm = wrapper.vm as any;
    vm.form.name = 'srv';
    vm.form.host = '1.2.3.4';
    vm.form.username = 'admin';
    vm.form.auth_type = AuthType.Password;
    vm.form.password = 'pw';
    await nextTick();

    await vm.submit();

    const payload = wrapper.emitted('save')![0][0] as any;
    expect(payload.auth_type).toBe(AuthType.Password);
  });
});
