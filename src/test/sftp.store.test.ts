import { beforeEach, describe, expect, it, vi } from 'vitest';
import { createPinia, setActivePinia } from 'pinia';
import { invoke } from '@tauri-apps/api/core';
import { emitMockEvent, resetMockEvents } from '@tauri-apps/api/event';
import { useSftpStore } from '@/stores/sftp';
import { makeRemoteEntry, makeRemoteDir, makeTransferTask } from './fixtures';

describe('sftp store', () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    vi.mocked(invoke).mockReset();
    resetMockEvents();
  });

  // --- listDir ---
  it('listDir 成功时更新 entries 和 currentPath', async () => {
    vi.mocked(invoke).mockResolvedValueOnce([makeRemoteDir(), makeRemoteEntry()]);
    const store = useSftpStore();
    await store.listDir('session-1', '/var/log');
    const state = store.getState('session-1');
    expect(state?.currentPath).toBe('/var/log');
    expect(state?.entries).toHaveLength(2);
    expect(state?.loading).toBe(false);
  });

  it('listDir 失败时写入 error', async () => {
    vi.mocked(invoke).mockRejectedValueOnce(new Error('SFTP 通道错误'));
    const store = useSftpStore();
    await store.listDir('session-1', '/root');
    const state = store.getState('session-1');
    expect(state?.error).toContain('SFTP 通道错误');
    expect(state?.loading).toBe(false);
  });

  // --- per-session 隔离 ---
  it('两个 session 状态互不影响', async () => {
    vi.mocked(invoke)
      .mockResolvedValueOnce([makeRemoteEntry({ path: '/a/file' })])
      .mockResolvedValueOnce([makeRemoteEntry({ path: '/b/file' })]);
    const store = useSftpStore();
    await store.listDir('session-1', '/a');
    await store.listDir('session-2', '/b');
    expect(store.getState('session-1')?.entries[0].path).toBe('/a/file');
    expect(store.getState('session-2')?.entries[0].path).toBe('/b/file');
  });

  // --- 进度事件 ---
  it('sftp:progress 更新 transferred_bytes 和 speed_bps', async () => {
    const store = useSftpStore();
    const dispose = await store.initListeners();
    store._injectTask('session-1', makeTransferTask({ status: 'Running', task_id: 'task-1' }));
    emitMockEvent('sftp:progress', {
      task_id: 'task-1', session_id: 'session-1',
      transferred_bytes: 25600, total_bytes: 51200, speed_bps: 1024000,
    });
    const task = store.getState('session-1')?.tasks.get('task-1');
    expect(task?.transferred_bytes).toBe(25600);
    expect(task?.speed_bps).toBe(1024000);
    dispose();
  });

  it('终态任务收到 sftp:progress 时忽略，不回退进度', async () => {
    const store = useSftpStore();
    const dispose = await store.initListeners();
    store._injectTask('session-1', makeTransferTask({
      task_id: 'task-1', status: 'Done',
      transferred_bytes: 51200, total_bytes: 51200,
    }));
    emitMockEvent('sftp:progress', {
      task_id: 'task-1', session_id: 'session-1',
      transferred_bytes: 100, total_bytes: 51200, speed_bps: 0,
    });
    const task = store.getState('session-1')?.tasks.get('task-1');
    expect(task?.transferred_bytes).toBe(51200);
    dispose();
  });

  // --- task_status 事件 ---
  it('sftp:task_status = Done 时 transferred_bytes 强制等于 total_bytes', async () => {
    const store = useSftpStore();
    const dispose = await store.initListeners();
    store._injectTask('session-1', makeTransferTask({ task_id: 'task-1', status: 'Running', transferred_bytes: 50000 }));
    emitMockEvent('sftp:task_status', {
      task_id: 'task-1', session_id: 'session-1', status: 'Done', error_message: null,
    });
    const task = store.getState('session-1')?.tasks.get('task-1');
    expect(task?.status).toBe('Done');
    expect(task?.transferred_bytes).toBe(51200);
    dispose();
  });

  it('sftp:task_status = Failed 时写入 error_message', async () => {
    const store = useSftpStore();
    const dispose = await store.initListeners();
    store._injectTask('session-1', makeTransferTask({ task_id: 'task-1', status: 'Running' }));
    emitMockEvent('sftp:task_status', {
      task_id: 'task-1', session_id: 'session-1', status: 'Failed', error_message: '网络中断',
    });
    const task = store.getState('session-1')?.tasks.get('task-1');
    expect(task?.status).toBe('Failed');
    expect(task?.error_message).toBe('网络中断');
    dispose();
  });

  it('sftp:task_status = Cancelled 时 error_message 为 null', async () => {
    const store = useSftpStore();
    const dispose = await store.initListeners();
    store._injectTask('session-1', makeTransferTask({ task_id: 'task-1', status: 'Running' }));
    emitMockEvent('sftp:task_status', {
      task_id: 'task-1', session_id: 'session-1', status: 'Cancelled', error_message: null,
    });
    const task = store.getState('session-1')?.tasks.get('task-1');
    expect(task?.status).toBe('Cancelled');
    expect(task?.error_message).toBeNull();
    dispose();
  });

  // --- cancelTask ---
  it('cancelTask 调用 invoke sftp_cancel_task', async () => {
    vi.mocked(invoke).mockResolvedValueOnce(undefined);
    const store = useSftpStore();
    await store.cancelTask('task-1');
    expect(invoke).toHaveBeenCalledWith('sftp_cancel_task', { taskId: 'task-1' });
  });

  // --- clearSession ---
  it('clearSession 清理指定 session 所有状态', async () => {
    vi.mocked(invoke).mockResolvedValueOnce([makeRemoteEntry()]);
    const store = useSftpStore();
    await store.listDir('session-1', '/var/log');
    store.clearSession('session-1');
    expect(store.getState('session-1')).toBeUndefined();
  });

  // --- toggleSelect ---
  it('toggleSelect 切换文件选中状态', () => {
    const store = useSftpStore();
    store.toggleSelect('session-1', '/var/log/syslog');
    expect(store.getState('session-1')?.selectedPaths.has('/var/log/syslog')).toBe(true);
    store.toggleSelect('session-1', '/var/log/syslog');
    expect(store.getState('session-1')?.selectedPaths.has('/var/log/syslog')).toBe(false);
  });
});
