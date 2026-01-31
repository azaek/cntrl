/**
 * API response types matching Bridge REST/WS responses
 * These types align with apps/bridge/src-tauri/src/server/types.rs
 */

// ============ Static System Info (/api/system) ============

export interface OsInfo {
  name: string;
  version: string;
  build: string;
  arch: string;
}

export interface CpuInfo {
  manufacturer: string;
  brand: string;
  cores: number;
  physical_cores: number;
  base_speed: number;
}

export interface GpuInfo {
  manufacturer: string;
  brand: string;
  memory_total: number;
}

export interface MemoryInfo {
  total: number;
  slots: number;
}

export interface DiskInfo {
  fs: string;
  type: string;
  size: number;
  mount: string;
}

export interface NetworkInfo {
  name: string;
  mac: string;
  ipv4: string;
  ipv6: string;
}

/**
 * Static system info response from GET /api/system
 */
export interface SystemInfo {
  hostname: string;
  platform: string;
  os: OsInfo;
  cpu: CpuInfo;
  gpu: GpuInfo | null;
  memory: MemoryInfo;
  disks: DiskInfo[];
  network: NetworkInfo | null;
}

// ============ Dynamic Usage (/api/usage) ============

export interface CpuUsage {
  current_load: number;
  current_temp: number;
  current_speed: number;
}

export interface MemoryUsage {
  used: number;
  free: number;
  used_percent: number;
}

export interface GpuUsage {
  current_load: number;
  current_temp: number;
  current_memory: number;
}

export interface DiskUsage {
  fs: string;
  used: number;
  available: number;
  used_percent: number;
}

/**
 * Dynamic usage response from GET /api/usage
 */
export interface SystemUsage {
  uptime: number;
  cpu: CpuUsage;
  memory: MemoryUsage;
  gpu: GpuUsage | null;
  disks: DiskUsage[];
}

// ============ Stream/WebSocket Stats ============

export interface NetworkUsage {
  bytes_sent: number;
  bytes_recv: number;
}

/**
 * Real-time stats from WS system_stats event
 */
export interface StreamPayload {
  timestamp: number;
  uptime: number;
  cpu?: CpuUsage;
  memory?: MemoryUsage;
  gpu?: GpuUsage;
  disks?: DiskUsage[];
  network?: NetworkUsage;
  media?: MediaStatus;
}

// ============ Media ============

/**
 * Media status from GET /api/media/status or WS media_update
 */
export interface MediaStatus {
  status: string;
  volume: number | null;
  muted: boolean | null;
  playing: boolean | null;
  title: string | null;
  artist: string | null;
  supports_ctrl: boolean;
}

/**
 * Media control action types
 */
export type MediaActionType =
  | "play"
  | "pause"
  | "play_pause"
  | "next"
  | "prev"
  | "previous"
  | "set_volume"
  | "volume_up"
  | "volume_down"
  | "mute"
  | "unmute"
  | "toggle_mute";

/**
 * Media control request
 */
export interface MediaAction {
  action: MediaActionType;
  value?: number;
}

// ============ Processes ============

/**
 * Aggregated process info from GET /api/processes
 */
export interface ProcessInfo {
  name: string;
  count: number;
  memory: number;
  memory_mb: number;
  cpu_time: number;
}

/**
 * Detailed process info from GET /api/processes/{name}
 */
export interface ProcessDetail {
  pid: number;
  name: string;
  memory: number;
  cpu: number;
  title: string | null;
  has_window: boolean;
}

/**
 * Process list payload from WS process_list event
 */
export interface ProcessListPayload {
  timestamp: number;
  processes: ProcessInfo[];
  total_count: number;
}

/**
 * Operation feedback from WS media_feedback/process_feedback
 */
export interface OperationFeedback {
  success: boolean;
  action: string;
  message?: string;
  pid?: number;
  name?: string;
}

// ============ Power ============

export type PowerAction = "shutdown" | "restart" | "sleep" | "hibernate";

// ============ Status ============

/**
 * Status check response from GET /api/status
 */
export interface StatusResponse {
  status: string;
}

/**
 * Alias for SystemInfo (used by client.ts)
 */
export type SystemInfoResponse = SystemInfo;

// ============ Ping Result ============

/**
 * Result from ping operation
 */
export interface PingResult {
  online: boolean;
  responseTime: number | null;
  lastChecked: number;
}
