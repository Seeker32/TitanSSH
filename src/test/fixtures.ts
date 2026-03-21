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
    auth_type: AuthType.Password,
    password_ref: 'titanssh-host-1-password',
    remark: 'primary',
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
    auth_type: AuthType.Password,
    password: 'secret',
    remark: 'primary',
    ...overrides,
  };
}

/** 生成测试用 SessionInfo（无 active/isHome 字段，created_at 为毫秒时间戳） */
export function makeSession(overrides: Partial<SessionInfo> = {}): SessionInfo {
  return {
    session_id: 'session-1',
    host_id: 'host-1',
    host: '10.0.0.8',
    port: 22,
    username: 'root',
    status: SessionStatus.Connecting,
    created_at: 1_710_000_000_000,
    ...overrides,
  };
}

/** 生成测试用 MonitorSnapshot（timestamp 为毫秒时间戳） */
export function makeSnapshot(overrides: Partial<MonitorSnapshot> = {}): MonitorSnapshot {
  return {
    session_id: 'session-1',
    timestamp: 1_710_000_120_000,
    cpu_usage: 21.5,
    memory_usage: 25.0,
    disk_usage: 40.0,
    disk_available_bytes: 300 * 1024 * 1024 * 1024,
    disk_total_bytes: 500 * 1024 * 1024 * 1024,
    ...overrides,
  };
}

/** 生成测试用 TaskInfo（初始状态为 Pending，task_type 为 monitor） */
export function makeTaskInfo(overrides: Partial<TaskInfo> = {}): TaskInfo {
  return {
    task_id: 'task-1',
    task_type: 'monitor',
    session_id: 'session-1',
    status: TaskStatus.Pending,
    created_at: 1_710_000_000_000,
    ...overrides,
  };
}

/** 生成测试用 RemoteEntry（文件） */
export function makeRemoteEntry(overrides: Partial<RemoteEntry> = {}): RemoteEntry {
  return {
    name: 'syslog',
    path: '/var/log/syslog',
    is_dir: false,
    size: 51200,
    modified_at: 1_710_000_120_000,
    permissions: 'rw-r--r--',
    ...overrides,
  };
}

/** 生成测试用 RemoteEntry（目录） */
export function makeRemoteDir(overrides: Partial<RemoteEntry> = {}): RemoteEntry {
  return {
    name: 'nginx',
    path: '/var/log/nginx',
    is_dir: true,
    size: 0,
    modified_at: 1_710_000_000_000,
    permissions: 'rwxr-xr-x',
    ...overrides,
  };
}

/** 生成测试用 TransferTask（下载，Pending 状态） */
export function makeTransferTask(overrides: Partial<TransferTask> = {}): TransferTask {
  return {
    task_id: 'task-sftp-1',
    session_id: 'session-1',
    transfer_type: 'Download',
    remote_path: '/var/log/syslog',
    local_path: '/Users/user/Downloads/syslog',
    file_name: 'syslog',
    total_bytes: 51200,
    transferred_bytes: 0,
    speed_bps: 0,
    status: 'Pending',
    error_message: null,
    created_at: 1_710_000_000_000,
    ...overrides,
  };
}
