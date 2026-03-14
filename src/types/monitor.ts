export interface ServerStatus {
  session_id: string;
  ip: string;
  uptime_text: string;
  load1: number;
  load5: number;
  load15: number;
  cpu_percent: number;
  memory_used_mb: number;
  memory_total_mb: number;
  memory_percent: number;
  swap_used_mb: number;
  swap_total_mb: number;
  swap_percent: number;
  updated_at: number;
}
