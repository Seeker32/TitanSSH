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
    let hostsStore = [host, groupedHost];
    const session = { sessionId: 'session-1', hostId: 'host-1', host: '10.0.0.8', port: 22, username: 'root', status: 'Connecting', createdAt: Date.now() };
    const task = { taskId: 'task-1', taskType: 'monitor', sessionId: 'session-1', status: 'Pending', createdAt: Date.now() };
    // 与正式 typed event contract（src/types/sftp.ts TransferTask）一致：结构化 error 字段
    const transfer = { taskId: 'transfer-1', sessionId: 'session-1', transferType: 'Download', remotePath: '/syslog', localPath: '/tmp/syslog', fileName: 'syslog', totalBytes: 100, transferredBytes: 0, speedBps: 0, status: 'Pending', error: null, createdAt: Date.now() };
    // sftp_task_snapshot 的权威响应；测试可在打开会话前注入
    let snapshotTasks: unknown[] = [];
    // 待消费的失败注入队列：按 command 匹配，命中则让 invoke 以结构化错误拒绝
    const failQueue: Array<{ command: string; error: { code: string; detail?: string } }> = [];
    // 主机身份确认建模（issue #31）：pending challenge 与 gated capability 等待者。
    // 真实后端：未知主机在握手后、认证前阻断 Terminal/SFTP/Monitoring 连接并派发 challenge；
    // mock 以 pendingChallenges 充当同一后端权威，sftp_list_dir/start_monitoring 在决定前不返回。
    const pendingChallenges = new Map<string, {
      challenge: { challengeId: string; sessionId: string };
      waiters: Array<{ command: string; resolve: () => void; reject: (error: unknown) => void }>;
    }>();
    // 每个 gated capability 的决定终局（接受 code=null），供断言三 capability 服从同一决定
    const identityWaiterResults: Array<{ command: string; code: string | null }> = [];
    // 测试开启后，open_session 立即派发未知主机 challenge（模拟后端连接到达校验门）
    let autoHostIdentity = false;
    // 持久化信任记录（issue #32）：保存成功后同 endpoint 的新 Session 静默放行不再提示
    const savedTrust = new Map<string, { algorithm: string; fingerprint: string }>();
    /** 精确 endpoint 键：host + port（与后端信任记录归属一致）。 */
    const endpointKey = (host: string, port: number) => `${host}:${port}`;
    /** 向所有已注册监听器派发结构化 Tauri 事件。 */
    const emitEvent = (name: string, payload: unknown) => {
      if (name === 'host-identity:challenge' && payload && typeof payload === 'object' && 'sessionId' in payload) {
        const challenge = payload as { challengeId: string; sessionId: string };
        pendingChallenges.set(challenge.sessionId, { challenge, waiters: [] });
      }
      listeners.get(name)?.forEach((id) => callbacks.get(id)?.({ event: name, id, payload }));
    };
    /** 命中 pending challenge 时返回等待决定的门控 Promise；否则直接返回正常响应。 */
    const gateOnIdentity = (command: string, sessionId: string, produce: () => unknown) => {
      const entry = pendingChallenges.get(sessionId);
      if (!entry) return Promise.resolve(produce());
      return new Promise((resolve, reject) => {
        entry.waiters.push({ command, resolve: () => resolve(produce()), reject });
      });
    };
    /** 按 challengeId 取出 pending challenge（跨 session 匹配，与命令契约一致）。 */
    const pendingByChallengeId = (challengeId: unknown) =>
      [...pendingChallenges.values()].find((entry) => entry.challenge.challengeId === challengeId);
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
        const failIndex = failQueue.findIndex((entry) => entry.command === command);
        if (failIndex >= 0) {
          const { error } = failQueue.splice(failIndex, 1)[0];
          throw error;
        }
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
        if (command === 'list_hosts') return hostsStore;
        if (command === 'save_host') {
          const request = args.request as { id: string; name?: string; group?: string };
          const index = hostsStore.findIndex((item) => item.id === request.id);
          if (index >= 0) {
            hostsStore = hostsStore.map((item) => item.id === request.id ? { ...item, ...request } : item);
          } else {
            hostsStore = [...hostsStore, { ...host, ...request }];
          }
          return hostsStore;
        }
        if (command === 'close_session') {
          // 关闭标签/会话：取消该 Session 的 pending challenge 与全部等待者（不进入认证）
          const entry = pendingChallenges.get(String(args.sessionId));
          if (entry) {
            pendingChallenges.delete(entry.challenge.sessionId);
            entry.waiters.forEach((waiter) => {
              identityWaiterResults.push({ command: waiter.command, code: 'HostKeyVerificationCancelled' });
              waiter.reject({ code: 'HostKeyVerificationCancelled', detail: String(args.sessionId) });
            });
          }
          return undefined;
        }
        if (command === 'open_session') {
          // 未知主机：连接到达统一校验门，mock 后端派发 challenge；认证前不返回 capability 数据。
          // 已保存信任记录的 endpoint：静默放行，不派发 challenge，直接进入 Connected
          if (autoHostIdentity && !savedTrust.has(endpointKey('10.0.0.8', 22))) {
            emitEvent('host-identity:challenge', {
              challengeId: 'challenge-auto',
              sessionId: 'session-1',
              host: '10.0.0.8',
              port: 22,
              keyAlgorithm: 'ssh-ed25519',
              fingerprint: 'SHA256:aGVscG1l',
              timestamp: Date.now(),
            });
            // 终端连接到达主机身份验证阶段（会话注册后再派发，投影方可接收）
            setTimeout(() => {
              emitEvent('session:progress', { sessionId: 'session-1', phase: 'VerifyingHostKey', timestamp: Date.now() });
            }, 0);
          } else if (autoHostIdentity) {
            setTimeout(() => {
              emitEvent('session:status', { sessionId: 'session-1', status: 'Connected', message: null });
            }, 0);
          }
          return session;
        }
        if (command === 'accept_and_save_host_identity') {
          const entry = pendingByChallengeId(args.challengeId);
          if (!entry) throw { code: 'HostKeyChallengeNotFound', detail: String(args.challengeId) };
          pendingChallenges.delete(entry.challenge.sessionId);
          entry.waiters.forEach((waiter) => {
            identityWaiterResults.push({ command: waiter.command, code: null });
            waiter.resolve();
          });
          // 持久化信任记录：后续新 Session 同 endpoint 不再产生 challenge
          savedTrust.set(endpointKey('10.0.0.8', 22), { algorithm: 'ssh-ed25519', fingerprint: 'SHA256:aGVscG1l' });
          // 后端放行认证：终端会话进入 Connected
          emitEvent('session:status', { sessionId: entry.challenge.sessionId, status: 'Connected', message: null });
          return undefined;
        }
        if (command === 'accept_host_identity') {
          const entry = pendingByChallengeId(args.challengeId);
          if (!entry) throw { code: 'HostKeyChallengeNotFound', detail: String(args.challengeId) };
          pendingChallenges.delete(entry.challenge.sessionId);
          entry.waiters.forEach((waiter) => {
            identityWaiterResults.push({ command: waiter.command, code: null });
            waiter.resolve();
          });
          // 后端放行认证：终端会话进入 Connected
          emitEvent('session:status', { sessionId: entry.challenge.sessionId, status: 'Connected', message: null });
          return undefined;
        }
        if (command === 'reject_host_identity') {
          const entry = pendingByChallengeId(args.challengeId);
          if (!entry) throw { code: 'HostKeyChallengeNotFound', detail: String(args.challengeId) };
          pendingChallenges.delete(entry.challenge.sessionId);
          entry.waiters.forEach((waiter) => {
            identityWaiterResults.push({ command: waiter.command, code: 'HostKeyRejected' });
            waiter.reject({ code: 'HostKeyRejected', detail: '10.0.0.8:22' });
          });
          return undefined;
        }
        if (command === 'delete_host') {
          hostsStore = hostsStore.filter((item) => item.id !== (args.hostId as string));
          return hostsStore;
        }
        if (command === 'start_monitoring') return gateOnIdentity('start_monitoring', String(args.sessionId), () => task);
        if (command === 'sftp_list_dir') return gateOnIdentity('sftp_list_dir', String(args.sessionId), () => [{ name: 'syslog', path: '/syslog', isDir: false, size: 100, modifiedAt: Date.now(), permissions: 'rw-r--r--' }]);
        if (command === 'sftp_download') return transfer;
        if (command === 'sftp_upload') return { ...transfer, taskId: 'transfer-2', transferType: 'Upload', fileName: 'upload.txt' };
        if (command === 'sftp_task_snapshot') return snapshotTasks;
        if (command === 'sftp_clear_terminal_tasks') return undefined;
        return undefined;
      },
    };
    Object.assign(window, {
      __TAURI_INTERNALS__: internals,
      __TAURI_TEST__: {
        calls,
        /** 向所有已注册监听器派发结构化 Tauri 事件。 */
        emit(name: string, payload: unknown) {
          emitEvent(name, payload);
        },
        /** 开启主机身份确认建模：open_session 后 mock 后端派发未知主机 challenge 并阻塞 capability。 */
        enableHostIdentity() {
          autoHostIdentity = true;
        },
        /** 当前等待统一决定的 gated capability 数（sftp_list_dir / start_monitoring）。 */
        pendingIdentityWaits() {
          return [...pendingChallenges.values()].reduce((total, entry) => total + entry.waiters.length, 0);
        },
        /** 每个 gated capability 的决定终局，证明三 capability 服从同一决定。 */
        identityWaiterResults() {
          return [...identityWaiterResults];
        },
        /** 已保存的持久化信任记录数（验收：保存后新 Session 不再提示）。 */
        savedTrustCount() {
          return savedTrust.size;
        },
        /** 让下一次匹配 command 的 invoke 以结构化错误拒绝（与后端 AppErrorInfo 契约一致）。 */
        failNext(command: string, error: { code: string; detail?: string }) {
          failQueue.push({ command, error });
        },
        /** 设置 sftp_task_snapshot 的权威响应（打开会话前注入）。 */
        setSnapshotTasks(tasks: unknown[]) {
          snapshotTasks = tasks;
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

test('分组可重命名且删除后主机归入未分组', async ({ page }) => {
  await page.goto('/');
  const sidebar = page.locator('.sidebar');
  const header = sidebar.getByTestId('group-header-prod-env');
  await header.hover();
  await header.getByTestId('group-rename-btn').click();
  const input = page.getByTestId('group-rename-input');
  await input.fill('prod-eu');
  await input.press('Enter');
  await expect(sidebar.getByText('prod-eu')).toBeVisible();
  await expect(sidebar.getByTestId('group-header-prod-env')).toHaveCount(0);
  await expect(sidebar.getByText('staging')).toBeVisible();
  const renamed = sidebar.getByTestId('group-header-prod-eu');
  await renamed.hover();
  await renamed.getByTestId('group-delete-btn').click();
  await expect(sidebar.getByTestId('group-header-prod-eu')).toHaveCount(0);
  const ungrouped = sidebar.getByTestId('group-header-ungrouped');
  await expect(ungrouped).toBeVisible();
  await expect(ungrouped.getByText('staging', { exact: true })).toHaveCount(0);
  await expect(sidebar.locator('.host-group-body').filter({ hasText: 'staging' })).toBeVisible();
});

test('监视条可折叠为状态点并刷新后保持', async ({ page }) => {
  await page.goto('/');
  await expect(page.getByText('服务器状态', { exact: true })).toBeVisible();
  await page.getByTestId('monitor-collapse-btn').click();
  await expect(page.getByText('服务器状态', { exact: true })).toBeHidden();
  await expect(page.getByTestId('monitor-strip')).toBeVisible();
  await page.reload();
  await expect(page.getByTestId('monitor-strip')).toBeVisible();
  await page.getByTestId('monitor-strip').click();
  await expect(page.getByText('服务器状态', { exact: true })).toBeVisible();
});

test('无会话时显示空态页且隐藏标签栏', async ({ page }) => {
  await page.goto('/');
  await expect(page.getByText(/选择左侧主机/)).toBeVisible();
  await expect(page.locator('.tabs-area')).toHaveCount(0);
  await page.locator('.empty-state').getByRole('button', { name: '新建主机' }).click();
  await expect(page.getByText('新建连接')).toBeVisible();
  await page.getByRole('button', { name: /取\s*消/ }).click();
});

test('侧栏主机卡片可编辑与删除', async ({ page }) => {
  await page.goto('/');
  const sidebar = page.locator('.sidebar');
  const card = sidebar.getByTestId('host-card-host-1');
  await card.hover();
  await card.getByTestId('host-edit-btn').click();
  await expect(page.getByText('编辑连接')).toBeVisible();
  await page.getByRole('button', { name: /取\s*消/ }).click();
  await card.hover();
  await card.getByTestId('host-delete-btn').click();
  await expect(sidebar.getByTestId('host-card-host-1')).toHaveCount(0);
  const deleteCalls = await page.evaluate(() => (window as unknown as { __TAURI_TEST__: { calls: Array<{ command: string }> } }).__TAURI_TEST__.calls.filter((call) => call.command === 'delete_host').length);
  expect(deleteCalls).toBe(1);
});

test('SFTP 面板字号统一密度体系', async ({ page }) => {
  await page.goto('/');
  await page.locator('.sidebar').getByTestId('host-card-host-1').dblclick();
  await page.evaluate(() => (window as unknown as { __TAURI_TEST__: { emit: (name: string, payload: unknown) => void } }).__TAURI_TEST__.emit('session:status', { sessionId: 'session-1', status: 'Connected', message: null }));
  await page.getByText('syslog').click();
  await expect(page.locator('.file-row').first()).toHaveCSS('font-size', '12px');
  await page.getByRole('button', { name: '下载' }).click();
  await page.getByTestId('tab-queue').click();
  await expect(page.locator('.task-name').first()).toHaveCSS('font-size', '12px');
});

test('SSH、终端、监控与文件传输形成完整闭环', async ({ page }) => {
  await page.goto('/');
  await page.locator('.sidebar').getByTestId('host-card-host-1').dblclick();
  await expect(page.getByRole('tab', { name: /root@10.0.0.8/ })).toBeVisible();
  await page.evaluate(() => (window as unknown as { __TAURI_TEST__: { emit: (name: string, payload: unknown) => void } }).__TAURI_TEST__.emit('session:status', { sessionId: 'session-1', status: 'Connected', message: null }));
  await page.evaluate(() => (window as unknown as { __TAURI_TEST__: { emit: (name: string, payload: unknown) => void } }).__TAURI_TEST__.emit('monitor:snapshot', {
    sessionId: 'session-1', timestamp: Date.now(), cpuUsage: 21.5, memoryUsage: 25, diskUsage: 40,
    diskAvailableBytes: 322122547200, diskTotalBytes: 536870912000,
    network: { available: true, interfaces: [
      { name: 'eth0', receiveBytesPerSecond: 1024, transmitBytesPerSecond: 512 },
      { name: 'eth1', receiveBytesPerSecond: 2048, transmitBytesPerSecond: 1024 },
    ] },
  }));
  await expect(page.getByText('21.5%')).toBeVisible();
  await expect(page.getByText('1.0 KB/s')).toBeVisible();
  await expect(page.getByLabel('网卡接口')).toHaveValue('eth0');
  await page.getByLabel('网卡接口').selectOption('eth1');
  await expect(page.getByText('2.0 KB/s')).toBeVisible();
  await page.evaluate(() => (window as unknown as { __TAURI_TEST__: { emit: (name: string, payload: unknown) => void } }).__TAURI_TEST__.emit('monitor:snapshot', {
    sessionId: 'session-1', timestamp: Date.now() + 1_000, cpuUsage: 21.5, memoryUsage: 25, diskUsage: 40,
    diskAvailableBytes: 322122547200, diskTotalBytes: 536870912000,
    network: { available: true, interfaces: [
      { name: 'eth0', receiveBytesPerSecond: 1024, transmitBytesPerSecond: 512 },
      { name: 'eth1', receiveBytesPerSecond: 3072, transmitBytesPerSecond: 1536 },
    ] },
  }));
  await expect(page.getByRole('img', { name: '最近一分钟网卡速率趋势' })).toBeVisible();
  await expect(page.getByText('3.0 KB/s')).toBeVisible();
  await page.getByText('syslog').click();
  await page.getByRole('button', { name: '下载' }).click();
  await page.getByTestId('tab-queue').click();
  await expect(page.getByText('等待中')).toBeVisible();
  const commands = await page.evaluate(() => (window as unknown as { __TAURI_TEST__: { calls: Array<{ command: string }> } }).__TAURI_TEST__.calls.map((call) => call.command));
  expect(commands).toEqual(expect.arrayContaining(['open_session', 'start_monitoring', 'sftp_list_dir', 'sftp_download']));
});

test('终端标签独立呈现连接生命周期：阶段、错误与关闭', async ({ page }) => {
  await page.goto('/');
  await page.locator('.sidebar').getByTestId('host-card-host-1').dblclick();
  const emit = (name: string, payload: unknown) => page.evaluate(([eventName, eventPayload]) => {
    (window as unknown as { __TAURI_TEST__: { emit: (event: string, value: unknown) => void } }).__TAURI_TEST__.emit(eventName as string, eventPayload);
  }, [name, payload] as const);

  // Connecting：加载动画与当前阶段，且无任何操作按钮
  await emit('session:progress', { sessionId: 'session-1', phase: 'ConnectingTcp', timestamp: Date.now() });
  await expect(page.locator('.terminal-overlay--connecting')).toContainText('正在建立 TCP 连接...');
  await expect(page.locator('.terminal-overlay .spinner')).toBeVisible();

  // Connected：覆盖层消失，终端可交互
  await emit('session:status', { sessionId: 'session-1', status: 'Connected', message: null });
  await expect(page.locator('.terminal-overlay')).toHaveCount(0);

  // 连接错误回到所属标签，仅提供关闭标签操作
  await emit('session:status', {
    sessionId: 'session-1', status: 'Error',
    error: { code: 'SshConnectionError', detail: 'connection refused' },
  });
  await expect(page.getByRole('alert')).toContainText('SSH 连接失败');
  await page.getByRole('button', { name: '关闭标签' }).click();
  const closed = await page.evaluate(() => (window as unknown as { __TAURI_TEST__: { calls: Array<{ command: string; args: Record<string, unknown> }> } }).__TAURI_TEST__.calls
    .filter((call) => call.command === 'close_session'));
  expect(closed).toEqual([{ command: 'close_session', args: { sessionId: 'session-1' } }]);
  await expect(page.getByRole('alert')).toHaveCount(0);
  await expect(page.locator('.empty-state')).toBeVisible();
});

test('失败状态可见且传输任务可以重试', async ({ page }) => {
  await page.goto('/');
  await page.locator('.sidebar').getByTestId('host-card-host-1').dblclick();
  const emit = (name: string, payload: unknown) => page.evaluate(([eventName, eventPayload]) => {
    (window as unknown as { __TAURI_TEST__: { emit: (event: string, value: unknown) => void } }).__TAURI_TEST__.emit(eventName as string, eventPayload);
  }, [name, payload] as const);
  await emit('session:status', { sessionId: 'session-1', status: 'Timeout', message: null });
  await expect(page.locator('.dot-error')).toBeVisible();
  await emit('session:status', { sessionId: 'session-1', status: 'Connected', message: null });
  await page.getByText('syslog').click();
  await page.getByRole('button', { name: '下载' }).click();
  // 与正式 typed event contract 一致：结构化 error 字段
  await emit('sftp:task_status', { taskId: 'transfer-1', sessionId: 'session-1', status: 'Failed', error: { code: 'SftpTransferError', detail: 'network' } });
  await page.getByTestId('tab-queue').click();
  await expect(page.getByText('network')).toBeVisible();
  await page.getByTestId('retry-btn').click();
  const count = await page.evaluate(() => (window as unknown as { __TAURI_TEST__: { calls: Array<{ command: string }> } }).__TAURI_TEST__.calls.filter((call) => call.command === 'sftp_download').length);
  expect(count).toBe(2);
});

test('下载启动失败显示在文件浏览器错误区且不产生未处理异常', async ({ page }) => {
  await page.goto('/');
  await page.locator('.sidebar').getByTestId('host-card-host-1').dblclick();
  await page.evaluate(() => (window as unknown as { __TAURI_TEST__: { emit: (name: string, payload: unknown) => void } }).__TAURI_TEST__.emit('session:status', { sessionId: 'session-1', status: 'Connected', message: null }));
  await page.getByText('syslog').click();
  // 下一次 sftp_download invoke 以结构化错误拒绝
  await page.evaluate(() => (window as unknown as { __TAURI_TEST__: { failNext: (command: string, error: unknown) => void } }).__TAURI_TEST__.failNext('sftp_download', { code: 'SftpPathNotFound', detail: '/syslog' }));
  await page.getByRole('button', { name: '下载' }).click();
  await expect(page.locator('.state-msg--error')).toHaveText(/SFTP 路径不存在: \/syslog/);
  await page.getByTestId('tab-queue').click();
  await expect(page.getByText('暂无传输任务')).toBeVisible();
});

test('取消 invoke 失败显示在对应任务行', async ({ page }) => {
  await page.goto('/');
  await page.locator('.sidebar').getByTestId('host-card-host-1').dblclick();
  await page.evaluate(() => (window as unknown as { __TAURI_TEST__: { emit: (name: string, payload: unknown) => void } }).__TAURI_TEST__.emit('session:status', { sessionId: 'session-1', status: 'Connected', message: null }));
  await page.getByText('syslog').click();
  await page.getByRole('button', { name: '下载' }).click();
  await page.getByTestId('tab-queue').click();
  await expect(page.getByText('等待中')).toBeVisible();
  await page.evaluate(() => (window as unknown as { __TAURI_TEST__: { failNext: (command: string, error: unknown) => void } }).__TAURI_TEST__.failNext('sftp_cancel_task', { code: 'SftpTaskNotFound', detail: 'transfer-1' }));
  await page.getByTestId('cancel-btn').click();
  await expect(page.getByTestId('task-action-error')).toHaveText(/SFTP 任务不存在: transfer-1/);
});

test('重试 invoke 失败仅显示在原任务行，不写入文件浏览器错误区', async ({ page }) => {
  await page.goto('/');
  await page.locator('.sidebar').getByTestId('host-card-host-1').dblclick();
  const emit = (name: string, payload: unknown) => page.evaluate(([eventName, eventPayload]) => {
    (window as unknown as { __TAURI_TEST__: { emit: (event: string, value: unknown) => void } }).__TAURI_TEST__.emit(eventName as string, eventPayload);
  }, [name, payload] as const);
  await emit('session:status', { sessionId: 'session-1', status: 'Connected', message: null });
  await page.getByText('syslog').click();
  await page.getByRole('button', { name: '下载' }).click();
  await emit('sftp:task_status', { taskId: 'transfer-1', sessionId: 'session-1', status: 'Failed', error: { code: 'SftpTransferError', detail: 'network' } });
  await page.getByTestId('tab-queue').click();
  await expect(page.getByText('network')).toBeVisible();
  await page.evaluate(() => (window as unknown as { __TAURI_TEST__: { failNext: (command: string, error: unknown) => void } }).__TAURI_TEST__.failNext('sftp_download', { code: 'SftpPermissionDenied', detail: '/var/log' }));
  await page.getByTestId('retry-btn').click();
  await expect(page.getByTestId('task-action-error')).toHaveText(/SFTP 权限拒绝: \/var\/log/);
  await page.getByTestId('tab-explorer').click();
  await expect(page.locator('.state-msg--error')).toHaveCount(0);
});

test('会话打开后从后端权威任务快照恢复传输队列', async ({ page }) => {
  await page.goto('/');
  // 打开会话前注入后端快照：模拟错过的事件已沉淀为权威终态
  await page.evaluate(() => (window as unknown as {
    __TAURI_TEST__: { setSnapshotTasks: (tasks: unknown[]) => void };
  }).__TAURI_TEST__.setSnapshotTasks([{
    taskId: 'transfer-snap', sessionId: 'session-1', transferType: 'Download',
    remotePath: '/syslog', localPath: '/tmp/syslog', fileName: 'snapshot.log',
    totalBytes: 100, transferredBytes: 100, speedBps: 0, status: 'Done', error: null, createdAt: Date.now(),
  }]));
  await page.locator('.sidebar').getByTestId('host-card-host-1').dblclick();
  await page.getByTestId('tab-queue').click();
  await expect(page.getByText('snapshot.log')).toBeVisible();
  await expect(page.getByText('完成')).toBeVisible();
  const snapshotCalls = await page.evaluate(() => (window as unknown as {
    __TAURI_TEST__: { calls: Array<{ command: string; args: Record<string, unknown> }> };
  }).__TAURI_TEST__.calls.filter((call) => call.command === 'sftp_task_snapshot'));
  expect(snapshotCalls).toHaveLength(1);
  expect(snapshotCalls[0].args).toEqual({ sessionId: 'session-1' });
});

test('清除已结束按钮清空终态任务且保留活动任务', async ({ page }) => {
  await page.goto('/');
  await page.locator('.sidebar').getByTestId('host-card-host-1').dblclick();
  const emit = (name: string, payload: unknown) => page.evaluate(([eventName, eventPayload]) => {
    (window as unknown as { __TAURI_TEST__: { emit: (event: string, value: unknown) => void } }).__TAURI_TEST__.emit(eventName as string, eventPayload);
  }, [name, payload] as const);
  await emit('session:status', { sessionId: 'session-1', status: 'Connected', message: null });
  await page.getByText('syslog').click();
  await page.getByRole('button', { name: '下载' }).click();
  await page.getByRole('button', { name: '上传' }).click();
  await emit('sftp:task_status', { taskId: 'transfer-1', sessionId: 'session-1', status: 'Done', error: null });
  await emit('sftp:task_status', { taskId: 'transfer-2', sessionId: 'session-1', status: 'Running', error: null });
  await page.getByTestId('tab-queue').click();
  await expect(page.getByTestId('clear-terminal-btn')).toBeVisible();
  await page.getByTestId('clear-terminal-btn').click();
  await expect(page.getByText('upload.txt')).toBeVisible();
  await expect(page.getByText('syslog')).toHaveCount(0);
  await expect(page.getByTestId('clear-terminal-btn')).toHaveCount(0);
  const clearCalls = await page.evaluate(() => (window as unknown as {
    __TAURI_TEST__: { calls: Array<{ command: string }> };
  }).__TAURI_TEST__.calls.filter((call) => call.command === 'sftp_clear_terminal_tasks'));
  expect(clearCalls).toHaveLength(1);
});

test('首次主机身份：内联确认卡仅本次接受，三 capability 统一等待决定', async ({ page }) => {
  await page.goto('/');
  await page.evaluate(() => (window as unknown as {
    __TAURI_TEST__: { enableHostIdentity: () => void };
  }).__TAURI_TEST__.enableHostIdentity());
  await page.locator('.sidebar').getByTestId('host-card-host-1').dblclick();

  // 等待决定期间：标签保持 Connecting 状态点，确认卡内联于终端区域而非全局 Modal
  await expect(page.locator('.tab .dot-connecting')).toHaveCount(1);
  const card = page.locator('.terminal-pane').getByTestId('host-identity-card');
  await expect(card).toBeVisible();
  await expect(card).toContainText('10.0.0.8:22');
  await expect(card).toContainText('ssh-ed25519');
  await expect(card).toContainText('SHA256:aGVscG1l');
  // 等待期间展示主机身份验证阶段；终端不可交互（不绕过校验）
  await expect(card).toContainText('正在验证主机身份...');
  await expect(page.locator('.terminal-view')).toHaveAttribute('data-interactive', 'false');
  await expect(page.locator('.ant-modal')).toHaveCount(0);

  // 三 capability 不绕过：SFTP 目录与监控任务均阻塞在统一校验门后
  await expect(page.getByText('syslog')).toHaveCount(0);
  await expect(page.locator('.state-msg')).toContainText('加载中');
  expect(await page.evaluate(() => (window as unknown as {
    __TAURI_TEST__: { pendingIdentityWaits: () => number };
  }).__TAURI_TEST__.pendingIdentityWaits())).toBe(2);

  // 仅本次接受：同一决定放行全部等待者
  await page.getByTestId('host-identity-accept').click();
  const acceptCalls = await page.evaluate(() => (window as unknown as {
    __TAURI_TEST__: { calls: Array<{ command: string; args: Record<string, unknown> }> };
  }).__TAURI_TEST__.calls.filter((call) => call.command === 'accept_host_identity'));
  expect(acceptCalls).toEqual([{ command: 'accept_host_identity', args: { challengeId: 'challenge-auto' } }]);
  await expect(card).toHaveCount(0);
  await expect(page.locator('.terminal-overlay')).toHaveCount(0);
  await expect(page.getByText('syslog')).toBeVisible();
  await expect(page.locator('.terminal-view')).toHaveAttribute('data-interactive', 'true');
  expect(await page.evaluate(() => (window as unknown as {
    __TAURI_TEST__: { pendingIdentityWaits: () => number };
  }).__TAURI_TEST__.pendingIdentityWaits())).toBe(0);
  const results = await page.evaluate(() => (window as unknown as {
    __TAURI_TEST__: { identityWaiterResults: () => Array<{ command: string; code: string | null }> };
  }).__TAURI_TEST__.identityWaiterResults());
  expect(results).toEqual([
    { command: 'sftp_list_dir', code: null },
    { command: 'start_monitoring', code: null },
  ]);
});

test('拒绝未知主机身份：三 capability 以 HostKeyRejected 失败并关闭整个 Session', async ({ page }) => {
  await page.goto('/');
  await page.evaluate(() => (window as unknown as {
    __TAURI_TEST__: { enableHostIdentity: () => void };
  }).__TAURI_TEST__.enableHostIdentity());
  await page.locator('.sidebar').getByTestId('host-card-host-1').dblclick();
  await expect(page.getByTestId('host-identity-card')).toBeVisible();

  await page.getByTestId('host-identity-reject').click();
  const rejectCalls = await page.evaluate(() => (window as unknown as {
    __TAURI_TEST__: { calls: Array<{ command: string; args: Record<string, unknown> }> };
  }).__TAURI_TEST__.calls.filter((call) => call.command === 'reject_host_identity'));
  expect(rejectCalls).toEqual([{ command: 'reject_host_identity', args: { challengeId: 'challenge-auto' } }]);

  // 同一决定：全部等待者以 HostKeyRejected 失败，不进入认证；Session 整体关闭
  await expect(page.locator('.empty-state')).toBeVisible();
  const results = await page.evaluate(() => (window as unknown as {
    __TAURI_TEST__: { identityWaiterResults: () => Array<{ command: string; code: string | null }> };
  }).__TAURI_TEST__.identityWaiterResults());
  expect(results).toEqual([
    { command: 'sftp_list_dir', code: 'HostKeyRejected' },
    { command: 'start_monitoring', code: 'HostKeyRejected' },
  ]);
  await expect(page.getByTestId('host-identity-card')).toHaveCount(0);
  await expect(page.getByRole('tab', { name: /root@10.0.0.8/ })).toHaveCount(0);
  // 后端在拒绝命令内完成 teardown，前端不得重复 close_session（无冗余 invoke）
  const closeCalls = await page.evaluate(() => (window as unknown as {
    __TAURI_TEST__: { calls: Array<{ command: string; args: Record<string, unknown> }> };
  }).__TAURI_TEST__.calls.filter((call) => call.command === 'close_session'));
  expect(closeCalls).toHaveLength(0);
});

test('等待确认期间关闭标签取消验证：不发起认证并取消全部等待者', async ({ page }) => {
  await page.goto('/');
  await page.evaluate(() => (window as unknown as {
    __TAURI_TEST__: { enableHostIdentity: () => void };
  }).__TAURI_TEST__.enableHostIdentity());
  await page.locator('.sidebar').getByTestId('host-card-host-1').dblclick();
  await expect(page.getByTestId('host-identity-card')).toBeVisible();

  // 关闭标签：不发起任何接受/拒绝决定，等待者全部取消，Session 关闭
  await page.locator('.tab .close-btn').click();
  await expect(page.locator('.empty-state')).toBeVisible();
  const decisions = await page.evaluate(() => (window as unknown as {
    __TAURI_TEST__: { calls: Array<{ command: string }> };
  }).__TAURI_TEST__.calls.filter((call) => call.command === 'accept_host_identity' || call.command === 'reject_host_identity'));
  expect(decisions).toHaveLength(0);
  const results = await page.evaluate(() => (window as unknown as {
    __TAURI_TEST__: { identityWaiterResults: () => Array<{ command: string; code: string | null }> };
  }).__TAURI_TEST__.identityWaiterResults());
  expect(results).toEqual([
    { command: 'sftp_list_dir', code: 'HostKeyVerificationCancelled' },
    { command: 'start_monitoring', code: 'HostKeyVerificationCancelled' },
  ]);
  const closeCalls = await page.evaluate(() => (window as unknown as {
    __TAURI_TEST__: { calls: Array<{ command: string; args: Record<string, unknown> }> };
  }).__TAURI_TEST__.calls.filter((call) => call.command === 'close_session'));
  expect(closeCalls).toEqual([{ command: 'close_session', args: { sessionId: 'session-1' } }]);
});

test('接受并保存：保存信任记录后新 Session 静默放行，不再提示', async ({ page }) => {
  await page.goto('/');
  await page.evaluate(() => (window as unknown as {
    __TAURI_TEST__: { enableHostIdentity: () => void };
  }).__TAURI_TEST__.enableHostIdentity());
  await page.locator('.sidebar').getByTestId('host-card-host-1').dblclick();
  const card = page.locator('.terminal-pane').getByTestId('host-identity-card');
  await expect(card).toBeVisible();

  // 接受并保存：同一决定放行全部等待者，信任记录持久化
  await page.getByTestId('host-identity-save').click();
  const saveCalls = await page.evaluate(() => (window as unknown as {
    __TAURI_TEST__: { calls: Array<{ command: string; args: Record<string, unknown> }> };
  }).__TAURI_TEST__.calls.filter((call) => call.command === 'accept_and_save_host_identity'));
  expect(saveCalls).toEqual([{ command: 'accept_and_save_host_identity', args: { challengeId: 'challenge-auto' } }]);
  await expect(card).toHaveCount(0);
  await expect(page.getByText('syslog')).toBeVisible();
  await expect(page.locator('.terminal-view')).toHaveAttribute('data-interactive', 'true');
  expect(await page.evaluate(() => (window as unknown as {
    __TAURI_TEST__: { savedTrustCount: () => number };
  }).__TAURI_TEST__.savedTrustCount())).toBe(1);

  // 关闭后重开：同 endpoint 已保存，静默放行进入 Connected，不产生 challenge
  await page.locator('.tab .close-btn').click();
  await expect(page.locator('.empty-state')).toBeVisible();
  await page.locator('.sidebar').getByTestId('host-card-host-1').dblclick();
  await expect(page.locator('.terminal-pane').getByTestId('host-identity-card')).toHaveCount(0);
  await expect(page.locator('.terminal-view')).toHaveAttribute('data-interactive', 'true');
  const identityCalls = await page.evaluate(() => (window as unknown as {
    __TAURI_TEST__: { calls: Array<{ command: string }> };
  }).__TAURI_TEST__.calls.filter((call) => call.command.startsWith('accept_') || call.command === 'reject_host_identity'));
  expect(identityCalls).toEqual([
    { command: 'accept_and_save_host_identity', args: { challengeId: 'challenge-auto' } },
  ]);
});

test('保存失败：challenge 保持未决并显示结构化错误，可改选仅本次接受', async ({ page }) => {
  await page.goto('/');
  await page.evaluate((setup) => {
    const bridge = (window as unknown as {
      __TAURI_TEST__: { enableHostIdentity: () => void; failNext: (command: string, error: { code: string; detail?: string }) => void };
    }).__TAURI_TEST__;
    bridge.enableHostIdentity();
    bridge.failNext('accept_and_save_host_identity', { code: 'HostKeySaveFailed', detail: 'write denied' });
  }, undefined);
  await page.locator('.sidebar').getByTestId('host-card-host-1').dblclick();
  const card = page.locator('.terminal-pane').getByTestId('host-identity-card');
  await expect(card).toBeVisible();

  await page.getByTestId('host-identity-save').click();
  // 保存失败：确认卡保持未决，结构化错误显示在所属标签；等待者仍阻塞在校验门后
  await expect(card).toBeVisible();
  await expect(card.getByTestId('host-identity-save-error')).toContainText('write denied');
  expect(await page.evaluate(() => (window as unknown as {
    __TAURI_TEST__: { pendingIdentityWaits: () => number };
  }).__TAURI_TEST__.pendingIdentityWaits())).toBe(2);
  // 失败绝不自动降级为临时信任：没有 accept_host_identity 调用
  const acceptCallsAfterFail = await page.evaluate(() => (window as unknown as {
    __TAURI_TEST__: { calls: Array<{ command: string }> };
  }).__TAURI_TEST__.calls.filter((call) => call.command === 'accept_host_identity'));
  expect(acceptCallsAfterFail).toHaveLength(0);

  // 用户明确改选仅本次接受：正常解决并放行全部等待者
  await page.getByTestId('host-identity-accept').click();
  await expect(card).toHaveCount(0);
  await expect(page.getByText('syslog')).toBeVisible();
  await expect(page.locator('.terminal-view')).toHaveAttribute('data-interactive', 'true');
});
