import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow, LogicalSize } from "@tauri-apps/api/window";
import { check, Update } from "@tauri-apps/plugin-updater";

// ============================================================================
// Type Definitions (must match Rust config.rs)
// ============================================================================

export interface TopicConfig {
  enabled: boolean;
  interval_ms: number;
}

export interface WebSocketConfig {
  stats: TopicConfig;
  media: TopicConfig;
  processes: TopicConfig;
}

export interface ServerConfig {
  port: number;
  host: string;
}

export interface DisplayConfig {
  hostname: string;
}

export interface FeaturesConfig {
  enable_shutdown: boolean;
  enable_restart: boolean;
  enable_hibernate: boolean;
  enable_sleep: boolean;
  enable_system: boolean;
  enable_usage: boolean;
  enable_stats: boolean;
  enable_media: boolean;
  enable_processes: boolean;
  enable_stream: boolean;
  enable_autostart: boolean;
}

export interface StatsConfig {
  gpu_enabled: boolean;
  disk_cache_seconds: number;
  stream_interval_seconds: number;
}

export interface AuthConfig {
  enabled: boolean;
  api_key: string | null;
  allowed_ips: string[];
  blocked_ips: string[];
}

export interface Config {
  server: ServerConfig;
  display: DisplayConfig;
  features: FeaturesConfig;
  stats: StatsConfig;
  auth: AuthConfig;
  websocket: WebSocketConfig;
}
// For compatibility with UI code that expects capitalized keys (if any) or flat structure
// But the Rust struct was refactored to be nested.
// Check App.tsx usages: `cfg()?.Server?.Port` -> this needs update in App.tsx too!

export const loadConfig = async ({
  onFinish = () => {},
  onError = () => {},
  onSuccess = () => {},
}: {
  onFinish?: () => void;
  onError?: (e: Error) => void;
  onSuccess?: (c: Config) => void;
}) => {
  try {
    const c = await invoke<Config>("get_config");
    if (c) {
      onSuccess(c);
    }
  } catch (e) {
    console.error("Failed to load config:", e);
    if (onError) onError(e as any);
  } finally {
    onFinish();
  }
};

export type ServerStatus =
  | { status: "Starting" }
  | { status: "Running" }
  | { status: "Stopped" }
  | { status: "Error"; message: string };

export interface ServerState {
  status: ServerStatus;
  port: number;
}

export const getServerStatus = async (): Promise<ServerState> => {
  try {
    return await invoke<ServerState>("get_server_status");
  } catch (e) {
    console.error("Failed to get server status:", e);
    return { status: { status: "Error", message: String(e) }, port: 0 };
  }
};

export const getAppVersion = async (): Promise<string> => {
  try {
    return await invoke<string>("get_app_version");
  } catch (e) {
    console.error("Failed to get app version:", e);
    return "unknown";
  }
};

export type Feature =
  | "shutdown"
  | "restart"
  | "hibernate"
  | "sleep"
  | "system"
  | "usage"
  | "stats"
  | "media"
  | "processes"
  | "stream"
  | "autostart";

export const toggleFeature = async (feature: Feature) => {
  try {
    await invoke("toggle_feature", { feature });
  } catch (e) {
    console.error("Failed to toggle feature:", e);
  }
};

export const reloadConfig = async () => {
  try {
    await invoke("reload_config");
  } catch (e) {
    console.error("Failed to reload config:", e);
  }
};

export const openConfig = async () => {
  try {
    await invoke("open_config_dir");
  } catch (e) {
    console.error("Failed to open config dir:", e);
  }
};

export const stopService = async () => {
  try {
    await invoke("stop_service");
  } catch (e) {
    console.error("Failed to stop service:", e);
  }
};

export const setWindowSize = async (width: number, height: number) => {
  try {
    console.log(`Resizing to ${width}x${height}`);
    await getCurrentWindow().setSize(new LogicalSize(width, height));
  } catch (e) {
    console.error("Failed to set window size:", e);
  }
};

// macOS rounded corners functions
export const enableRoundedCorners = async () => {
  try {
    await invoke("enable_rounded_corners");
  } catch (e) {
    console.error("Failed to enable rounded corners:", e);
  }
};

export const enableModernWindowStyle = async () => {
  try {
    await invoke("enable_modern_window_style");
  } catch (e) {
    console.error("Failed to enable modern window style:", e);
  }
};

export const repositionTrafficLights = async (x: number, y: number) => {
  try {
    await invoke("reposition_traffic_lights", { x, y });
  } catch (e) {
    console.error("Failed to reposition traffic lights:", e);
  }
};

// ============================================================================
// Server Control Functions
// ============================================================================

/**
 * Restart the server with current config (stops and starts server)
 */
export const restartServer = async (): Promise<Config | null> => {
  try {
    return await invoke<Config>("reload_config");
  } catch (e) {
    console.error("Failed to restart server:", e);
    return null;
  }
};

/**
 * Start the server (if stopped). Currently uses reload_config which handles start.
 */
export const startServer = async (): Promise<Config | null> => {
  return restartServer();
};

// ============================================================================
// WebSocket Controls
// ============================================================================

export type WebSocketTopic = "stats" | "media" | "processes";

/**
 * Update WebSocket topic interval (in milliseconds)
 */
export const updateWsInterval = async (
  topic: WebSocketTopic,
  intervalMs: number,
): Promise<Config | null> => {
  try {
    return await invoke<Config>("update_ws_interval", { topic, intervalMs });
  } catch (e) {
    console.error(`Failed to update WS interval for ${topic}:`, e);
    return null;
  }
};

/**
 * Toggle WebSocket topic enabled state
 */
export const toggleWsTopic = async (topic: WebSocketTopic): Promise<Config | null> => {
  try {
    return await invoke<Config>("toggle_ws_topic", { topic });
  } catch (e) {
    console.error(`Failed to toggle WS topic ${topic}:`, e);
    return null;
  }
};

/**
 * Get active WebSocket connection count from server
 */
export const getWsClientCount = async (port: number): Promise<number> => {
  try {
    const response = await fetch(`http://127.0.0.1:${port}/api/clients`);
    if (response.ok) {
      const data = await response.json();
      return data.count ?? 0;
    }
    return 0;
  } catch (e) {
    console.error("Failed to get WS client count:", e);
    return 0;
  }
};

// ============================================================================
// Config Update Functions
// ============================================================================

/**
 * Update server port (requires restart to take effect)
 */
export const updateServerPort = async (port: number): Promise<Config | null> => {
  try {
    return await invoke<Config>("update_server_port", { port });
  } catch (e) {
    console.error("Failed to update server port:", e);
    return null;
  }
};

/**
 * Update server host/bind address (requires restart to take effect)
 */
export const updateServerHost = async (host: string): Promise<Config | null> => {
  try {
    return await invoke<Config>("update_server_host", { host });
  } catch (e) {
    console.error("Failed to update server host:", e);
    return null;
  }
};

/**
 * Update display hostname
 */
export const updateHostname = async (hostname: string): Promise<Config | null> => {
  try {
    return await invoke<Config>("update_hostname", { hostname });
  } catch (e) {
    console.error("Failed to update hostname:", e);
    return null;
  }
};

/**
 * Toggle authentication on/off
 */
export const toggleAuth = async (): Promise<Config | null> => {
  try {
    return await invoke<Config>("toggle_auth");
  } catch (e) {
    console.error("Failed to toggle auth:", e);
    return null;
  }
};

/**
 * Update API key for authentication
 */
export const updateApiKey = async (apiKey: string | null): Promise<Config | null> => {
  try {
    return await invoke<Config>("update_api_key", { apiKey });
  } catch (e) {
    console.error("Failed to update API key:", e);
    return null;
  }
};

/**
 * Generate a new random API key
 */
export const generateApiKey = async (): Promise<Config | null> => {
  try {
    return await invoke<Config>("generate_api_key");
  } catch (e) {
    console.error("Failed to generate API key:", e);
    return null;
  }
};

/**
 * Add an IP to the allowed list
 */
export const addAllowedIp = async (ip: string): Promise<Config | null> => {
  try {
    return await invoke<Config>("add_allowed_ip", { ip });
  } catch (e) {
    console.error("Failed to add allowed IP:", e);
    return null;
  }
};

/**
 * Remove an IP from the allowed list
 */
export const removeAllowedIp = async (ip: string): Promise<Config | null> => {
  try {
    return await invoke<Config>("remove_allowed_ip", { ip });
  } catch (e) {
    console.error("Failed to remove allowed IP:", e);
    return null;
  }
};

/**
 * Add an IP to the blocked list (blacklist)
 */
export const addBlockedIp = async (ip: string): Promise<Config | null> => {
  try {
    return await invoke<Config>("add_blocked_ip", { ip });
  } catch (e) {
    console.error("Failed to add blocked IP:", e);
    return null;
  }
};

/**
 * Remove an IP from the blocked list
 */
export const removeBlockedIp = async (ip: string): Promise<Config | null> => {
  try {
    return await invoke<Config>("remove_blocked_ip", { ip });
  } catch (e) {
    console.error("Failed to remove blocked IP:", e);
    return null;
  }
};

/**
 * Clear all blocked IPs
 */
export const clearBlockedIps = async (): Promise<Config | null> => {
  try {
    return await invoke<Config>("clear_blocked_ips");
  } catch (e) {
    console.error("Failed to clear blocked IPs:", e);
    return null;
  }
};

/**
 * Toggle GPU stats collection
 */
export const toggleGpuStats = async (): Promise<Config | null> => {
  try {
    return await invoke<Config>("toggle_gpu_stats");
  } catch (e) {
    console.error("Failed to toggle GPU stats:", e);
    return null;
  }
};

/**
 * Update disk cache duration (seconds)
 */
export const updateDiskCacheSeconds = async (seconds: number): Promise<Config | null> => {
  try {
    return await invoke<Config>("update_disk_cache_seconds", { seconds });
  } catch (e) {
    console.error("Failed to update disk cache seconds:", e);
    return null;
  }
};

/**
 * Update SSE stream interval (seconds)
 */
export const updateStreamInterval = async (seconds: number): Promise<Config | null> => {
  try {
    return await invoke<Config>("update_stream_interval", { seconds });
  } catch (e) {
    console.error("Failed to update stream interval:", e);
    return null;
  }
};

// ============================================================================
// Server Metrics & Status
// ============================================================================

export interface ServerMetrics {
  wsClients: number;
  uptime: number;
  requestsTotal?: number;
}

/**
 * Get server metrics including WS connections and uptime
 */
export const getServerMetrics = async (port: number): Promise<ServerMetrics> => {
  try {
    const [clientsRes, statusRes] = await Promise.all([
      fetch(`http://127.0.0.1:${port}/api/clients`).catch(() => null),
      fetch(`http://127.0.0.1:${port}/api/status`).catch(() => null),
    ]);

    const clients = clientsRes?.ok ? ((await clientsRes.json()).count ?? 0) : 0;
    const status = statusRes?.ok ? await statusRes.json() : null;

    return {
      wsClients: clients,
      uptime: status?.uptime ?? 0,
    };
  } catch (e) {
    console.error("Failed to get server metrics:", e);
    return { wsClients: 0, uptime: 0 };
  }
};

/**
 * Check if server is responding (health check)
 */
export const isServerHealthy = async (port: number): Promise<boolean> => {
  try {
    const response = await fetch(`http://127.0.0.1:${port}/api/status`, {
      signal: AbortSignal.timeout(2000),
    });
    return response.ok;
  } catch {
    return false;
  }
};

// ============================================================================
// Feature Toggle Helpers
// ============================================================================

export type FeatureKey =
  | "system"
  | "usage"
  | "stream"
  | "media"
  | "processes"
  | "autostart"
  | "shutdown"
  | "restart"
  | "hibernate"
  | "sleep";

/**
 * Toggle a specific feature and return updated config
 */
export const toggleFeatureWithResult = async (
  feature: FeatureKey,
): Promise<Config | null> => {
  try {
    return await invoke<Config>("toggle_feature", { feature });
  } catch (e) {
    console.error(`Failed to toggle feature ${feature}:`, e);
    return null;
  }
};

/**
 * Get current feature state
 */
export const getFeatureState = async (feature: FeatureKey): Promise<boolean | null> => {
  try {
    const config = await invoke<Config>("get_config");
    const key = `enable_${feature}` as keyof FeaturesConfig;
    return config.features[key] ?? null;
  } catch (e) {
    console.error(`Failed to get feature state for ${feature}:`, e);
    return null;
  }
};

// ============================================================================
// Batch Operations
// ============================================================================

/**
 * Update multiple config values at once
 */
export const updateConfig = async (
  updates: Partial<{
    server: Partial<ServerConfig>;
    display: Partial<DisplayConfig>;
    stats: Partial<StatsConfig>;
    websocket: Partial<{
      stats: Partial<TopicConfig>;
      media: Partial<TopicConfig>;
      processes: Partial<TopicConfig>;
    }>;
  }>,
): Promise<Config | null> => {
  try {
    return await invoke<Config>("update_config", { updates });
  } catch (e) {
    console.error("Failed to update config:", e);
    return null;
  }
};

// ============================================================================
// Utility Functions
// ============================================================================

/**
 * Format interval for display (ms to human readable)
 */
export const formatInterval = (ms: number): string => {
  if (ms < 1000) return `${ms}ms`;
  if (ms < 60000) return `${(ms / 1000).toFixed(1)}s`;
  return `${(ms / 60000).toFixed(1)}m`;
};

/**
 * Default WebSocket intervals
 */
export const WS_INTERVAL_PRESETS = {
  realtime: { stats: 500, media: 250, processes: 1000 },
  balanced: { stats: 1000, media: 500, processes: 3000 },
  lowPower: { stats: 5000, media: 2000, processes: 10000 },
} as const;

/**
 * Apply a WebSocket interval preset
 */
export const applyWsPreset = async (
  preset: keyof typeof WS_INTERVAL_PRESETS,
): Promise<boolean> => {
  const intervals = WS_INTERVAL_PRESETS[preset];
  try {
    await Promise.all([
      updateWsInterval("stats", intervals.stats),
      updateWsInterval("media", intervals.media),
      updateWsInterval("processes", intervals.processes),
    ]);
    return true;
  } catch (e) {
    console.error(`Failed to apply preset ${preset}:`, e);
    return false;
  }
};

/**
 * Get local network IPs for display
 */
export const getLocalIps = async (): Promise<string[]> => {
  try {
    return await invoke<string[]>("get_local_ips");
  } catch (e) {
    console.error("Failed to get local IPs:", e);
    return [];
  }
};

/**
 * Copy text to clipboard
 */
export const copyToClipboard = async (text: string): Promise<boolean> => {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch (e) {
    console.error("Failed to copy to clipboard:", e);
    return false;
  }
};

// ============================================================================
// App Updates
// ============================================================================

/**
 * Update channel is determined automatically based on current app version:
 * - Version contains "beta" → checks latest-beta.json
 * - Otherwise → checks latest-stable.json
 */

export interface UpdateInfo {
  version: string;
  currentVersion: string;
  body?: string;
  date?: string;
}

export interface UpdateProgress {
  downloaded: number;
  total: number | null;
}

export type UpdateStatus = "upToDate" | "available" | "error";

export type UpdateResult = {
  status: UpdateStatus;
  update?: Update;
  info?: UpdateInfo;
  message?: string;
};

/**
 * Check for application updates
 * Channel is determined automatically based on current app version:
 * - Beta versions (e.g., 1.0.0-beta.1) check latest-beta.json
 * - Stable versions (e.g., 1.0.0) check latest-stable.json
 */
export const checkForUpdates = async (): Promise<UpdateResult> => {
  try {
    const update = await check();

    if (!update) {
      return { status: "upToDate" };
    }

    return {
      status: "available",
      update,
      info: {
        version: update.version,
        currentVersion: update.currentVersion,
        body: update.body ?? undefined,
        date: update.date ?? undefined,
      },
    };
  } catch (e) {
    console.error("Failed to check for updates:", e);
    return { status: "error", message: String(e) };
  }
};

/**
 * Download and install an update
 * @param update - The update object from checkForUpdates
 * @param onProgress - Optional callback for download progress
 */
export const installUpdate = async (
  update: Update,
  onProgress?: (progress: UpdateProgress) => void,
): Promise<boolean> => {
  try {
    // Download with progress
    await update.downloadAndInstall((event) => {
      if (event.event === "Started" && onProgress) {
        onProgress({ downloaded: 0, total: event.data.contentLength ?? null });
      } else if (event.event === "Progress" && onProgress) {
        onProgress({
          downloaded: event.data.chunkLength,
          total: null,
        });
      } else if (event.event === "Finished") {
        console.log("Update download finished");
      }
    });

    return true;
  } catch (e) {
    console.error("Failed to install update:", e);
    return false;
  }
};

/**
 * Helper to determine if a version string is a beta version
 */
export const isBetaVersion = (version: string): boolean => {
  return version.includes("beta") || version.includes("alpha") || version.includes("rc");
};
