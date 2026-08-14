import { invoke } from '@tauri-apps/api/core';
import { create } from 'zustand';
import { toAppError, type AppErrorInfo } from '@/i18n';
import type { TrustedHostInfo } from '@/types/host-identity';

export type TrustedHostsStatus = 'idle' | 'loading' | 'ready' | 'error';

interface TrustedHostsState {
  status: TrustedHostsStatus;
  /** 后端按 host 字典序 + port 稳定排序返回的只读清单。 */
  hosts: TrustedHostInfo[];
  /** 读取失败的结构化错误；错误状态绝不伪装成空列表。 */
  error: AppErrorInfo | null;
  load: () => Promise<void>;
}

/** Settings“可信主机”只读清单的视图投影：每次进入该区域重新加载，
 *  保证保存/替换/自动清理后清单反映后端当前记录。React 只消费 typed JSON。 */
export const useTrustedHostsStore = create<TrustedHostsState>((set) => ({
  status: 'idle',
  hosts: [],
  error: null,
  async load() {
    set({ status: 'loading', error: null });
    try {
      const hosts = await invoke<TrustedHostInfo[]>('list_trusted_hosts');
      set({ status: 'ready', hosts, error: null });
    } catch (error) {
      set({ status: 'error', hosts: [], error: toAppError(error) });
    }
  },
}));
