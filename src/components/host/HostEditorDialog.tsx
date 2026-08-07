import { open as openFileDialog } from '@tauri-apps/plugin-dialog';
import { useEffect, useState } from 'react';
import { Button, Form, Input, InputNumber, Modal, Select } from 'antd';
import { AuthType, type HostConfig, type SaveHostRequest } from '@/types/host';

interface Props {
  open: boolean;
  editingHost: HostConfig | null;
  onClose: () => void;
  onSave: (request: SaveHostRequest) => void;
}

/** 根据编辑目标创建不含旧凭据的表单状态。 */
function formFromHost(host: HostConfig | null): SaveHostRequest {
  return {
    id: host?.id ?? '',
    name: host?.name ?? '',
    host: host?.host ?? '',
    port: host?.port ?? 22,
    username: host?.username ?? '',
    authType: host?.authType ?? AuthType.Password,
    password: '',
    privateKeyPath: host?.privateKeyPath ?? '',
    passphrase: '',
    remark: host?.remark ?? '',
    group: host?.group ?? '',
  };
}

/** 渲染新建或编辑主机的安全凭据表单。 */
export default function HostEditorDialog({ open, editingHost, onClose, onSave }: Props) {
  const [form, setForm] = useState<SaveHostRequest>(() => formFromHost(editingHost));

  useEffect(() => {
    if (open) setForm(formFromHost(editingHost));
  }, [open, editingHost]);

  /** 更新单个表单字段。 */
  function update<K extends keyof SaveHostRequest>(key: K, value: SaveHostRequest[K]) {
    setForm((current) => ({ ...current, [key]: value }));
  }

  /** 打开系统文件选择器选择私钥文件；取消选择时保持原值不变。 */
  async function pickPrivateKey() {
    const path = await openFileDialog({ multiple: false, directory: false, title: '选择私钥文件' });
    if (typeof path === 'string') update('privateKeyPath', path);
  }

  /** 提交表单并清除当前认证方式不使用的凭据字段。 */
  function submit() {
    onSave({
      ...form,
      id: form.id || crypto.randomUUID(),
      password: form.authType === AuthType.Password ? form.password : undefined,
      privateKeyPath: form.authType === AuthType.PrivateKey ? form.privateKeyPath : undefined,
      passphrase: form.authType === AuthType.PrivateKey ? form.passphrase : undefined,
    });
  }

  const textProps = { autoCapitalize: 'none', autoComplete: 'off', spellCheck: false };
  return <Modal open={open} title={editingHost ? '编辑连接' : '新建连接'} onCancel={onClose}
    onOk={submit} okText="保存连接" cancelText="取消" width={720} destroyOnHidden
    okButtonProps={{ disabled: form.authType === AuthType.PrivateKey && !form.privateKeyPath }}>
    <Form layout="vertical" className="host-form">
      <div className="form-grid">
        <Form.Item label="名称"><Input {...textProps} value={form.name} placeholder="生产服务器" onChange={(event) => update('name', event.target.value)} /></Form.Item>
        <Form.Item label="地址"><Input {...textProps} value={form.host} placeholder="192.168.1.12" onChange={(event) => update('host', event.target.value)} /></Form.Item>
        <Form.Item label="端口"><InputNumber min={1} max={65535} value={form.port} onChange={(value) => update('port', value ?? 22)} /></Form.Item>
        <Form.Item label="用户名"><Input {...textProps} value={form.username} placeholder="root" onChange={(event) => update('username', event.target.value)} /></Form.Item>
        <Form.Item label="认证方式"><Select value={form.authType} onChange={(value) => update('authType', value)}
          options={[{ label: '密码', value: AuthType.Password }, { label: '私钥', value: AuthType.PrivateKey }]} /></Form.Item>
        {form.authType === AuthType.Password ? <Form.Item label="密码"><Input.Password {...textProps} value={form.password}
          placeholder="留空则保持原密码不变" onChange={(event) => update('password', event.target.value)} /></Form.Item>
          : <Form.Item label="私钥路径" help={!form.privateKeyPath ? '请先选择私钥文件' : undefined}><Input {...textProps} readOnly value={form.privateKeyPath}
            placeholder="点击浏览选择私钥文件" addonAfter={<Button onClick={pickPrivateKey}>浏览…</Button>} /></Form.Item>}
        {form.authType === AuthType.PrivateKey && <Form.Item label="私钥口令"><Input.Password {...textProps}
          value={form.passphrase} placeholder="留空则保持原口令不变" onChange={(event) => update('passphrase', event.target.value)} /></Form.Item>}
        <Form.Item label="备注" className="form-full"><Input.TextArea {...textProps} rows={3} value={form.remark}
          placeholder="业务说明 / 环境标签" onChange={(event) => update('remark', event.target.value)} /></Form.Item>
      </div>
    </Form>
  </Modal>;
}
