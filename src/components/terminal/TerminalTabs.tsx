import { X } from 'lucide-react';
import type { MouseEvent } from 'react';
import { translate } from '@/i18n';
import { useLocaleStore } from '@/stores/locale';
import type { TabId, TabStripItem } from '@/stores/tab-view';

interface Props {
  /** 标签栏展示投影；组件只消费渲染所需字段。 */
  items: readonly TabStripItem[];
  onActivate: (tabId: TabId) => void;
  onClose: (tabId: TabId) => void;
}

/** 根据投影状态色返回现有样式类。 */
function statusDot(tone: TabStripItem['statusTone']) {
  if (tone === 'connected') return 'dot-connected';
  if (tone === 'connecting') return 'dot-connecting';
  if (tone === 'error') return 'dot-error';
  return 'dot-offline';
}

/** 渲染标签栏；标题、状态和活动态均来自 tab-view projection。 */
export default function TerminalTabs({ items, onActivate, onClose }: Props) {
  const locale = useLocaleStore((state) => state.locale);
  /** 关闭标签且不触发激活。 */
  function closeTab(event: MouseEvent, item: TabStripItem) {
    event.stopPropagation();
    onClose(item.id);
  }

  return (
    <div className="tab-bar" role="tablist">
      {items.map((item) => {
        return (
          <div key={item.id} className={`tab ${item.active ? 'active' : ''}`}
            role="tab" aria-selected={item.active} onClick={() => onActivate(item.id)}>
            <span className={`status-dot ${statusDot(item.statusTone)}`} />
            <span className="tab-label">{item.label}</span>
            <button type="button" className="close-btn" aria-label={translate(locale, 'tab.close', { name: item.label })}
              onClick={(event) => closeTab(event, item)}><X size={11} /></button>
          </div>
        );
      })}
    </div>
  );
}
