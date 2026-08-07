import { Button, Empty, Typography } from 'antd';
import type { MouseEvent } from 'react';
import type { HostConfig } from '@/types/host';

interface Props {
  hosts: HostConfig[];
  onOpen: (hostId: string) => void;
  onEdit: (hostId: string) => void;
  onRemove: (hostId: string) => void;
  onCreate: () => void;
}

/** 渲染首页主机快捷入口，并透传主机操作。 */
export default function HomeQuickActions({ hosts, onOpen, onEdit, onRemove, onCreate }: Props) {
  /** 阻止操作按钮触发主机连接。 */
  function runAction(event: MouseEvent, action: () => void) {
    event.stopPropagation();
    action();
  }

  return (
    <div className="home-quick-actions">
      <div className="home-host-scroll">
        {hosts.length === 0 ? (
          <Empty description="暂无保存的主机，点击下方按钮添加第一个 SSH 连接" />
        ) : (
          <div className="host-list">
            {hosts.map((host) => (
              <div key={host.id} className="host-btn" role="button" tabIndex={0}
                onClick={() => onOpen(host.id)} onKeyDown={(event) => event.key === 'Enter' && onOpen(host.id)}>
                <div className="host-main">
                  <div className="host-copy">
                    <Typography.Text strong>{host.name || host.host}</Typography.Text>
                    <Typography.Text type="secondary" className="host-address">
                      {host.username}@{host.host}:{host.port}
                    </Typography.Text>
                  </div>
                  <div className="host-actions">
                    <Button size="small" type="text" onClick={(event) => runAction(event, () => onEdit(host.id))}>编辑</Button>
                    <Button size="small" type="text" danger onClick={(event) => runAction(event, () => onRemove(host.id))}>删除</Button>
                  </div>
                </div>
              </div>
            ))}
          </div>
        )}
      </div>
      <div className="create-section"><Button block onClick={onCreate}>+ 新建主机</Button></div>
    </div>
  );
}
