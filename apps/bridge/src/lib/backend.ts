import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow, LogicalSize } from "@tauri-apps/api/window";
export interface Config {
  server: {
    port: number;
    host: string;
  };
  display: {
    hostname: string;
  };
  features: {
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
  };
  stats: {
    gpu_enabled: boolean;
    disk_cache_seconds: number;
    stream_interval_seconds: number;
  };
  auth: {
    enabled: boolean;
    api_key: string | null;
    allowed_ips: string[];
  };
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

export const toggleFeature = async (feature: string) => {
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
