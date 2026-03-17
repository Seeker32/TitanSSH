<script setup lang="ts">
import { computed, reactive, watch } from 'vue';
import {
  NModal, NCard, NForm, NFormItem, NInput, NInputNumber,
  NSelect, NButton, NSpace, NGrid, NGridItem,
} from 'naive-ui';
import { AuthType, type HostConfig, type SaveHostRequest } from '@/types/host';

const props = defineProps<{
  modelValue: boolean;
  editingHost: HostConfig | null;
}>();

const emit = defineEmits<{
  'update:modelValue': [boolean];
  /** 保存时提交 SaveHostRequest，含明文凭据，不含 ref 字段 */
  save: [SaveHostRequest];
}>();

/** 表单内部使用 SaveHostRequest 形状，含明文 password/passphrase */
const form = reactive<SaveHostRequest>({
  id: '',
  name: '',
  host: '',
  port: 22,
  username: '',
  auth_type: AuthType.Password,
  password: '',
  private_key_path: '',
  passphrase: '',
  remark: '',
});

const title = computed(() => (props.editingHost ? '编辑连接' : '新建连接'));

const authOptions = [
  { label: '密码', value: AuthType.Password },
  { label: '私钥', value: AuthType.PrivateKey },
];

/**
 * 监听对话框打开与编辑目标变化，重置表单。
 * 编辑时从 HostConfig（含 ref 字段）初始化非敏感字段，密码字段留空让用户重新输入。
 */
watch(
  () => [props.modelValue, props.editingHost],
  () => {
    const source = props.editingHost;
    Object.assign(form, {
      id: source?.id ?? '',
      name: source?.name ?? '',
      host: source?.host ?? '',
      port: source?.port ?? 22,
      username: source?.username ?? '',
      auth_type: source?.auth_type ?? AuthType.Password,
      // 编辑时密码字段留空，用户需重新输入
      password: '',
      private_key_path: source?.private_key_path ?? '',
      // 编辑时口令字段留空，用户需重新输入
      passphrase: '',
      remark: source?.remark ?? '',
    });
  },
  { immediate: true },
);

/** 关闭对话框 */
function close() {
  emit('update:modelValue', false);
}

/** 提交表单，emit SaveHostRequest，后端负责安全存储明文凭据 */
function submit() {
  const request: SaveHostRequest = {
    ...form,
    id: form.id || crypto.randomUUID(),
    password: form.auth_type === AuthType.Password ? form.password : undefined,
    private_key_path:
      form.auth_type === AuthType.PrivateKey ? form.private_key_path : undefined,
    passphrase: form.auth_type === AuthType.PrivateKey ? form.passphrase : undefined,
  };
  emit('save', request);
}
</script>

<template>
  <NModal :show="modelValue" :mask-closable="true" @update:show="emit('update:modelValue', $event)">
    <NCard :title="title" style="width: min(720px, calc(100vw - 32px))" :bordered="false" role="dialog">
      <NForm label-placement="top" label-width="auto">
        <NGrid :cols="2" :x-gap="16">
          <NGridItem>
            <NFormItem label="名称">
              <NInput v-model:value="form.name" placeholder="生产服务器" />
            </NFormItem>
          </NGridItem>
          <NGridItem>
            <NFormItem label="地址">
              <NInput v-model:value="form.host" placeholder="192.168.1.12" />
            </NFormItem>
          </NGridItem>
          <NGridItem>
            <NFormItem label="端口">
              <NInputNumber v-model:value="form.port" :min="1" :max="65535" style="width: 100%" />
            </NFormItem>
          </NGridItem>
          <NGridItem>
            <NFormItem label="用户名">
              <NInput v-model:value="form.username" placeholder="root" />
            </NFormItem>
          </NGridItem>
          <NGridItem>
            <NFormItem label="认证方式">
              <NSelect v-model:value="form.auth_type" :options="authOptions" />
            </NFormItem>
          </NGridItem>
          <NGridItem v-if="form.auth_type === AuthType.Password">
            <NFormItem label="密码">
              <NInput v-model:value="form.password" type="password" show-password-on="click" placeholder="留空则保持原密码不变" />
            </NFormItem>
          </NGridItem>
          <NGridItem v-else>
            <NFormItem label="私钥路径">
              <NInput v-model:value="form.private_key_path" placeholder="~/.ssh/id_rsa" />
            </NFormItem>
          </NGridItem>
          <NGridItem v-if="form.auth_type === AuthType.PrivateKey">
            <NFormItem label="私钥口令">
              <NInput v-model:value="form.passphrase" type="password" show-password-on="click" placeholder="留空则保持原口令不变" />
            </NFormItem>
          </NGridItem>
          <NGridItem :span="2">
            <NFormItem label="备注">
              <NInput v-model:value="form.remark" type="textarea" :rows="3" placeholder="业务说明 / 环境标签" />
            </NFormItem>
          </NGridItem>
        </NGrid>
      </NForm>
      <template #footer>
        <NSpace justify="end">
          <NButton @click="close">取消</NButton>
          <NButton type="primary" @click="submit">保存连接</NButton>
        </NSpace>
      </template>
    </NCard>
  </NModal>
</template>
