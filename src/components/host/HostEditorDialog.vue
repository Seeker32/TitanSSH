<script setup lang="ts">
import { computed, reactive, watch } from 'vue';
import { AuthType, type HostConfig } from '@/types/host';

const props = defineProps<{
  modelValue: boolean;
  editingHost: HostConfig | null;
}>();

const emit = defineEmits<{
  'update:modelValue': [boolean];
  save: [HostConfig];
}>();

const form = reactive<HostConfig>({
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
      password: source?.password ?? '',
      private_key_path: source?.private_key_path ?? '',
      passphrase: source?.passphrase ?? '',
      remark: source?.remark ?? '',
    });
  },
  { immediate: true },
);

function close() {
  emit('update:modelValue', false);
}

function submit() {
  emit('save', {
    ...form,
    id: form.id || crypto.randomUUID(),
    password: form.auth_type === AuthType.Password ? form.password : undefined,
    private_key_path:
      form.auth_type === AuthType.PrivateKey ? form.private_key_path : undefined,
    passphrase: form.auth_type === AuthType.PrivateKey ? form.passphrase : undefined,
  });
}
</script>

<template>
  <div v-if="modelValue" class="dialog-mask" @click.self="close">
    <div class="dialog">
      <div class="dialog-header">
        <h2>{{ title }}</h2>
        <button class="ghost" @click="close">关闭</button>
      </div>

      <div class="grid">
        <label>
          <span>名称</span>
          <input v-model="form.name" placeholder="生产服务器" />
        </label>
        <label>
          <span>地址</span>
          <input v-model="form.host" placeholder="192.168.1.12" />
        </label>
        <label>
          <span>端口</span>
          <input v-model.number="form.port" type="number" min="1" max="65535" />
        </label>
        <label>
          <span>用户名</span>
          <input v-model="form.username" placeholder="root" />
        </label>
        <label>
          <span>认证方式</span>
          <select v-model="form.auth_type">
            <option :value="AuthType.Password">密码</option>
            <option :value="AuthType.PrivateKey">私钥</option>
          </select>
        </label>
        <label v-if="form.auth_type === AuthType.Password">
          <span>密码</span>
          <input v-model="form.password" type="password" />
        </label>
        <label v-else>
          <span>私钥路径</span>
          <input v-model="form.private_key_path" placeholder="~/.ssh/id_rsa" />
        </label>
        <label v-if="form.auth_type === AuthType.PrivateKey">
          <span>私钥口令</span>
          <input v-model="form.passphrase" type="password" />
        </label>
        <label class="full">
          <span>备注</span>
          <textarea v-model="form.remark" rows="3" placeholder="业务说明 / 环境标签" />
        </label>
      </div>

      <div class="actions">
        <button class="ghost" @click="close">取消</button>
        <button class="primary" @click="submit">保存连接</button>
      </div>
    </div>
  </div>
</template>

<style scoped>
.dialog-mask {
  position: fixed;
  inset: 0;
  display: grid;
  place-items: center;
  background: rgba(5, 10, 15, 0.65);
  backdrop-filter: blur(8px);
  z-index: 20;
}

.dialog {
  width: min(760px, calc(100vw - 32px));
  padding: 24px;
  border: 1px solid var(--color-border);
  border-radius: 24px;
  background: var(--color-panel-bg);
  box-shadow: 0 30px 80px rgba(0, 0, 0, 0.38);
}

.dialog-header,
.actions {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
}

h2 {
  margin: 0;
  color: var(--color-text-primary);
}

.grid {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: 14px;
  margin: 18px 0 22px;
}

label {
  display: flex;
  flex-direction: column;
  gap: 8px;
  color: var(--color-text-secondary);
}

.full {
  grid-column: 1 / -1;
}

input,
select,
textarea {
  width: 100%;
  padding: 12px 14px;
  border: 1px solid var(--color-border);
  border-radius: 14px;
  color: var(--color-text-primary);
  background: var(--color-card-bg);
}

.ghost,
.primary {
  padding: 10px 16px;
  border-radius: 12px;
  border: 1px solid var(--color-border);
}

.ghost {
  color: var(--color-text-secondary);
  background: var(--color-card-bg);
}

.primary {
  color: var(--color-text-inverse);
  background: var(--color-accent);
}

@media (max-width: 700px) {
  .grid {
    grid-template-columns: 1fr;
  }
}
</style>
