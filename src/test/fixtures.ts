import { AuthType, type HostConfig } from '@/types/host';
import { SessionStatus, type SessionInfo } from '@/types/session';
import type { ServerStatus } from '@/types/monitor';

export function makeHost(overrides: Partial<HostConfig> = {}): HostConfig {
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

export function makeSession(overrides: Partial<SessionInfo> = {}): SessionInfo {
  return {
    session_id: 'session-1',
    host_id: 'host-1',
    host: '10.0.0.8',
    port: 22,
    username: 'root',
    status: SessionStatus.Connecting,
    created_at: 1_710_000_000,
    active: false,
    ...overrides,
  };
}

export function makeStatus(overrides: Partial<ServerStatus> = {}): ServerStatus {
  return {
    session_id: 'session-1',
    ip: '10.0.0.8',
    uptime_text: '3h 12m',
    load1: 0.3,
    load5: 0.4,
    load15: 0.6,
    cpu_percent: 21.5,
    memory_used_mb: 1024,
    memory_total_mb: 4096,
    memory_percent: 25,
    swap_used_mb: 256,
    swap_total_mb: 1024,
    swap_percent: 25,
    updated_at: 1_710_000_120,
    ...overrides,
  };
}
