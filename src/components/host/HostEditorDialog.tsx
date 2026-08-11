import { open as openFileDialog } from '@tauri-apps/plugin-dialog';
import { useEffect, useState } from 'react';
import { AutoComplete, Button, Form, Input, InputNumber, Modal, Select, Space } from 'antd';
import { AuthType, type HostConfig, type SaveHostRequest } from '@/types/host';
import { translate } from '@/i18n';
import { useLocaleStore } from '@/stores/locale';

interface Props {
  open: boolean;
  editingHost: HostConfig | null;
  /** 已有分组名列表，供分组字段下拉复用 */
  groups: string[];
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
export default function HostEditorDialog({ open, editingHost, groups, onClose, onSave }: Props) {
  const locale = useLocaleStore((state) => state.locale);
  const t = (key: Parameters<typeof translate>[1]) => translate(locale, key);
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
    const path = await openFileDialog({ multiple: false, directory: false, title: t('host.keyDialog') });
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
  return <Modal open={open} title={editingHost ? t('host.editConnection') : t('host.createConnection')} onCancel={onClose}
    onOk={submit} okText={t('host.save')} cancelText={t('common.cancel')} width={460} destroyOnHidden
    okButtonProps={{ disabled: form.authType === AuthType.PrivateKey && !form.privateKeyPath }}>
    <Form layout="vertical" className="host-form">
      <Form.Item label={t('host.name')}><Input {...textProps} value={form.name} placeholder={t('host.namePlaceholder')} onChange={(event) => update('name', event.target.value)} /></Form.Item>
      <Form.Item label={t('host.address')}><Input {...textProps} value={form.host} placeholder="192.168.1.12" onChange={(event) => update('host', event.target.value)} /></Form.Item>
      <Form.Item label={t('host.port')}><InputNumber min={1} max={65535} value={form.port} onChange={(value) => update('port', value ?? 22)} /></Form.Item>
      <Form.Item label={t('host.username')}><Input {...textProps} value={form.username} placeholder="root" onChange={(event) => update('username', event.target.value)} /></Form.Item>
      <Form.Item label={t('host.auth')}><Select aria-label={t('host.auth')} value={form.authType} onChange={(value) => update('authType', value)}
        options={[{ label: t('host.passwordAuth'), value: AuthType.Password }, { label: t('host.privateKeyAuth'), value: AuthType.PrivateKey }]} /></Form.Item>
      {form.authType === AuthType.Password ? <Form.Item label={t('host.password')}><Input.Password {...textProps} value={form.password}
        placeholder={t('host.passwordPlaceholder')} onChange={(event) => update('password', event.target.value)} /></Form.Item>
        : <Form.Item label={t('host.privateKeyPath')}>
          <Space.Compact block>
            <Input {...textProps} readOnly value={form.privateKeyPath}
              placeholder={t('host.keyPlaceholder')} />
            <Button onClick={pickPrivateKey}>{t('host.browse')}</Button>
          </Space.Compact>
        </Form.Item>}
      {form.authType === AuthType.PrivateKey && <Form.Item label={t('host.passphrase')}><Input.Password {...textProps}
        value={form.passphrase} placeholder={t('host.passphrasePlaceholder')} onChange={(event) => update('passphrase', event.target.value)} /></Form.Item>}
      <Form.Item label={t('host.group')}><AutoComplete aria-label={t('host.group')} value={form.group} placeholder={t('host.groupPlaceholder')}
        options={groups.map((name) => ({ value: name }))} filterOption
        onChange={(value) => update('group', value)} /></Form.Item>
      <Form.Item label={t('host.remark')}><Input.TextArea {...textProps} rows={3} value={form.remark}
        placeholder={t('host.remarkPlaceholder')} onChange={(event) => update('remark', event.target.value)} /></Form.Item>
    </Form>
  </Modal>;
}
