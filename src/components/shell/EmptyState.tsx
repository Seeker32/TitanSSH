import { Button, Empty } from 'antd';
import { Terminal } from 'lucide-react';
import { translate } from '@/i18n';
import { useLocaleStore } from '@/stores/locale';

interface Props {
  onCreateHost: () => void;
}

/** 无会话时主区空态页：引导文案与新建主机入口。 */
export default function EmptyState({ onCreateHost }: Props) {
  const locale = useLocaleStore((state) => state.locale);
  return (
    <Empty className="empty-state" image={<Terminal size={40} className="empty-state-icon" />}
      description={<>
        <p className="empty-state-title">{translate(locale, 'empty.title')}</p>
        <p className="empty-state-hint">{translate(locale, 'empty.hint')}</p>
      </>}>
      <Button type="primary" className="sidebar-create-btn" onClick={onCreateHost}>{translate(locale, 'host.create')}</Button>
    </Empty>
  );
}
