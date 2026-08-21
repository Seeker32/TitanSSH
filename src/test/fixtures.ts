import { AuthType, type HostConfig, type SaveHostRequest } from '@/types/host';
import { SessionStatus, type SessionInfo } from '@/types/session';
import type { MonitorSnapshot, TaskInfo } from '@/types/monitor';
import { TaskStatus } from '@/types/monitor';
import type { RemoteEntry, TransferTask } from '@/types/sftp';

/** 生成测试用 HostConfig（含 ref 字段，不含明文凭据） */
export function makeHost(overrides: Partial<HostConfig> = {}): HostConfig {
  return {
    id: 'host-1',
    name: 'prod',
    host: '10.0.0.8',
    port: 22,
    username: 'root',
    authType: AuthType.Password,
    passwordRef: 'titanssh-host-1-password',
    remark: 'primary',
    group: 'production',
    ...overrides,
  };
}

/** 生成测试用 SaveHostRequest（含明文凭据，用于提交场景） */
export function makeSaveHostRequest(overrides: Partial<SaveHostRequest> = {}): SaveHostRequest {
  return {
    id: 'host-1',
    name: 'prod',
    host: '10.0.0.8',
    port: 22,
    username: 'root',
    authType: AuthType.Password,
    password: 'secret',
    remark: 'primary',
    group: 'production',
    ...overrides,
  };
}

/** 生成测试用 SessionInfo（createdAt 为毫秒时间戳） */
export function makeSession(overrides: Partial<SessionInfo> = {}): SessionInfo {
  return {
    sessionId: 'session-1',
    hostId: 'host-1',
    host: '10.0.0.8',
    port: 22,
    username: 'root',
    status: SessionStatus.Connecting,
    createdAt: 1_710_000_000_000,
    ...overrides,
  };
}

/** 生成测试用 MonitorSnapshot（timestamp 为毫秒时间戳） */
export function makeSnapshot(overrides: Partial<MonitorSnapshot> = {}): MonitorSnapshot {
  return {
    sessionId: 'session-1',
    timestamp: 1_710_000_120_000,
    cpuUsage: 21.5,
    memoryUsage: 25.0,
    memoryUsedBytes: 2 * 1024 * 1024 * 1024,
    memoryTotalBytes: 8 * 1024 * 1024 * 1024,
    diskUsage: 40.0,
    diskAvailableBytes: 300 * 1024 * 1024 * 1024,
    diskTotalBytes: 500 * 1024 * 1024 * 1024,
    network: { available: true, interfaces: [] },
    ...overrides,
  };
}

/** 生成测试用 TaskInfo（初始状态为 Pending，task_type 为 monitor） */
export function makeTaskInfo(overrides: Partial<TaskInfo> = {}): TaskInfo {
  return {
    taskId: 'task-1',
    taskType: 'monitor',
    sessionId: 'session-1',
    status: TaskStatus.Pending,
    createdAt: 1_710_000_000_000,
    ...overrides,
  };
}

/** 生成测试用 RemoteEntry（文件） */
export function makeRemoteEntry(overrides: Partial<RemoteEntry> = {}): RemoteEntry {
  return {
    name: 'syslog',
    path: '/var/log/syslog',
    isDir: false,
    size: 51200,
    modifiedAt: 1_710_000_120_000,
    permissions: 'rw-r--r--',
    ...overrides,
  };
}

/** 生成测试用 RemoteEntry（目录） */
export function makeRemoteDir(overrides: Partial<RemoteEntry> = {}): RemoteEntry {
  return {
    name: 'nginx',
    path: '/var/log/nginx',
    isDir: true,
    size: 0,
    modifiedAt: 1_710_000_000_000,
    permissions: 'rwxr-xr-x',
    ...overrides,
  };
}

/** 生成测试用 TransferTask（下载，Pending 状态） */
export function makeTransferTask(overrides: Partial<TransferTask> = {}): TransferTask {
  return {
    taskId: 'task-sftp-1',
    sessionId: 'session-1',
    transferType: 'Download',
    remotePath: '/var/log/syslog',
    localPath: '/Users/user/Downloads/syslog',
    fileName: 'syslog',
    totalBytes: 51200,
    transferredBytes: 0,
    speedBps: 0,
    status: 'Pending',
    error: null,
    createdAt: 1_710_000_000_000,
    ...overrides,
  };
}
