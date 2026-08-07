import { expect, test } from '@playwright/test';

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    const callbacks = new Map<number, (event: unknown) => void>();
    const listeners = new Map<string, Set<number>>();
    const calls: Array<{ command: string; args: Record<string, unknown> }> = [];
    let callbackId = 0;
    let listenerId = 0;
    const host = { id: 'host-1', name: 'prod', host: '10.0.0.8', port: 22, username: 'root', authType: 'Password', passwordRef: 'secret-ref', remark: 'primary', group: '' };
    const groupedHost = { ...host, id: 'host-2', name: 'staging', host: '10.0.0.9', username: 'deploy', group: 'prod-env' };
    const session = { sessionId: 'session-1', hostId: 'host-1', host: '10.0.0.8', port: 22, username: 'root', status: 'Connecting', createdAt: Date.now() };
    const task = { taskId: 'task-1', taskType: 'monitor', sessionId: 'session-1', status: 'Pending', createdAt: Date.now() };
    const transfer = { taskId: 'transfer-1', sessionId: 'session-1', transferType: 'Download', remotePath: '/syslog', localPath: '/tmp/syslog', fileName: 'syslog', totalBytes: 100, transferredBytes: 0, speedBps: 0, status: 'Pending', errorMessage: null, createdAt: Date.now() };
    const internals = {
      /** 注册 Tauri 回调并返回数字句柄。 */
      transformCallback(callback?: (event: unknown) => void) {
        const id = ++callbackId;
        if (callback) callbacks.set(id, callback);
        return id;
      },
      /** 清理指定 Tauri 回调句柄。 */
      unregisterCallback(id: number) { callbacks.delete(id); },
      /** 模拟应用使用的 Tauri command 与插件调用。 */
      async invoke(command: string, args: Record<string, unknown> = {}) {
        calls.push({ command, args });
        if (command === 'plugin:event|listen') {
          const id = ++listenerId;
          const set = listeners.get(String(args.event)) ?? new Set<number>();
          set.add(Number(args.handler));
          listeners.set(String(args.event), set);
          return id;
        }
        if (command === 'plugin:event|unlisten') return undefined;
        if (command === 'plugin:dialog|save') return '/tmp/syslog';
        if (command === 'plugin:dialog|open') return '/tmp/upload.txt';
        if (command === 'list_hosts') return [host, groupedHost];
        if (command === 'save_host') return [{ ...host, name: args.request?.name ?? host.name, group: args.request?.group ?? '' }];
        if (command === 'open_session') return session;
        if (command === 'start_monitoring') return task;
        if (command === 'sftp_list_dir') return [{ name: 'syslog', path: '/syslog', isDir: false, size: 100, modifiedAt: Date.now(), permissions: 'rw-r--r--' }];
        if (command === 'sftp_download') return transfer;
        if (command === 'sftp_upload') return { ...transfer, taskId: 'transfer-2', transferType: 'Upload', fileName: 'upload.txt' };
        return undefined;
      },
    };
    Object.assign(window, {
      __TAURI_INTERNALS__: internals,
      __TAURI_TEST__: {
        calls,
        /** 向所有已注册监听器派发结构化 Tauri 事件。 */
        emit(name: string, payload: unknown) {
          listeners.get(name)?.forEach((id) => callbacks.get(id)?.({ event: name, id, payload }));
        },
      },
    });
  });
});

test('视觉基础：平铺 slate 背景且主题切换即时生效', async ({ page }) => {
  await page.goto('/');
  const style = await page.evaluate(() => {
    const body = document.body;
    return { bgImage: getComputedStyle(body).backgroundImage, theme: document.documentElement.dataset.theme };
  });
  expect(style.bgImage).toBe('none');
  const before = style.theme;
  await page.getByTestId('theme-toggle').click();
  const after = await page.evaluate(() => document.documentElement.dataset.theme);
  expect(after).not.toBe(before);
  const stillFlat = await page.evaluate(() => getComputedStyle(document.body).backgroundImage);
  expect(stillFlat).toBe('none');
});

test('侧栏主机列表：双击打开会话，单击不连接', async ({ page }) => {
  await page.goto('/');
  const openCalls = () => page.evaluate(() => (window as unknown as { __TAURI_TEST__: { calls: Array<{ command: string }> } }).__TAURI_TEST__.calls.filter((call) => call.command === 'open_session').length);
  const sidebar = page.locator('.sidebar');
  await expect(sidebar.getByText('prod', { exact: true })).toBeVisible();
  await sidebar.getByText('prod', { exact: true }).click();
  expect(await openCalls()).toBe(0);
  await sidebar.getByText('prod', { exact: true }).dblclick();
  expect(await openCalls()).toBe(1);
  await expect(page.getByRole('tab', { name: /root@10.0.0.8/ })).toBeVisible();
});

test('新建主机携带分组保存', async ({ page }) => {
  await page.goto('/');
  await page.getByLabel('新建主机').click();
  await page.getByPlaceholder('生产服务器').fill('web-1');
  await page.getByPlaceholder('192.168.1.12').fill('10.0.0.10');
  await page.getByRole('combobox', { name: '分组' }).fill('blue-team');
  await page.getByRole('button', { name: '保存连接' }).click();
  await expect(page.locator('.sidebar').getByText('web-1')).toBeVisible();
  const saveCall = await page.evaluate(() => (window as unknown as { __TAURI_TEST__: { calls: Array<{ command: string; args: { request?: { group?: string } } }> } }).__TAURI_TEST__.calls.find((call) => call.command === 'save_host'));
  expect(saveCall?.args?.request?.group).toBe('blue-team');
});

test('侧栏分组渲染、折叠且刷新后保持', async ({ page }) => {
  await page.goto('/');
  const sidebar = page.locator('.sidebar');
  await expect(sidebar.getByText('prod-env')).toBeVisible();
  await expect(sidebar.getByText('未分组')).toBeVisible();
  const headerNames = await sidebar.locator('.host-group-name').allTextContents();
  expect(headerNames).toEqual(['prod-env', '未分组']);
  await sidebar.getByText('prod-env').click();
  await expect(sidebar.getByText('staging')).toBeHidden();
  await page.reload();
  await expect(sidebar.getByText('staging')).toBeHidden();
  await sidebar.getByText('prod-env').click();
  await expect(sidebar.getByText('staging')).toBeVisible();
});

test('SSH、终端、监控与文件传输形成完整闭环', async ({ page }) => {
  await page.goto('/');
  await page.locator('.home-view').getByText('root@10.0.0.8:22').click();
  await expect(page.getByRole('tab', { name: /root@10.0.0.8/ })).toBeVisible();
  await page.evaluate(() => (window as unknown as { __TAURI_TEST__: { emit: (name: string, payload: unknown) => void } }).__TAURI_TEST__.emit('session:status', { sessionId: 'session-1', status: 'Connected', message: null }));
  await page.evaluate(() => (window as unknown as { __TAURI_TEST__: { emit: (name: string, payload: unknown) => void } }).__TAURI_TEST__.emit('monitor:snapshot', {
    sessionId: 'session-1', timestamp: Date.now(), cpuUsage: 21.5, memoryUsage: 25, diskUsage: 40,
    diskAvailableBytes: 322122547200, diskTotalBytes: 536870912000,
  }));
  await expect(page.getByText('21.5%')).toBeVisible();
  await page.getByText('syslog').click();
  await page.getByRole('button', { name: '下载' }).click();
  await page.getByTestId('tab-queue').click();
  await expect(page.getByText('等待中')).toBeVisible();
  const commands = await page.evaluate(() => (window as unknown as { __TAURI_TEST__: { calls: Array<{ command: string }> } }).__TAURI_TEST__.calls.map((call) => call.command));
  expect(commands).toEqual(expect.arrayContaining(['open_session', 'start_monitoring', 'sftp_list_dir', 'sftp_download']));
});

test('失败状态可见且传输任务可以重试', async ({ page }) => {
  await page.goto('/');
  await page.locator('.home-view').getByText('root@10.0.0.8:22').click();
  const emit = (name: string, payload: unknown) => page.evaluate(([eventName, eventPayload]) => {
    (window as unknown as { __TAURI_TEST__: { emit: (event: string, value: unknown) => void } }).__TAURI_TEST__.emit(eventName as string, eventPayload);
  }, [name, payload] as const);
  await emit('session:status', { sessionId: 'session-1', status: 'Timeout', message: null });
  await expect(page.locator('.dot-error')).toBeVisible();
  await emit('session:status', { sessionId: 'session-1', status: 'Connected', message: null });
  await page.getByText('syslog').click();
  await page.getByRole('button', { name: '下载' }).click();
  await emit('sftp:task_status', { taskId: 'transfer-1', sessionId: 'session-1', status: 'Failed', errorMessage: 'network' });
  await page.getByTestId('tab-queue').click();
  await expect(page.getByText('network')).toBeVisible();
  await page.getByTestId('retry-btn').click();
  const count = await page.evaluate(() => (window as unknown as { __TAURI_TEST__: { calls: Array<{ command: string }> } }).__TAURI_TEST__.calls.filter((call) => call.command === 'sftp_download').length);
  expect(count).toBe(2);
});
