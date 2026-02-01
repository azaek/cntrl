/**
 * Bridge device connection configuration
 */
export interface BridgeConfig {
  /** Bridge hostname or IP address */
  host: string;
  /** Bridge port (default: 9990) */
  port: number;
  /** Use secure connection (https/wss) */
  secure: boolean;
  /** API key for authentication (optional) */
  apiKey?: string;
}

/**
 * Apply defaults to a partial bridge config.
 */
export function parseBridgeConfig(
  input: Partial<BridgeConfig> & { host: string },
): BridgeConfig {
  return {
    host: input.host,
    port: input.port ?? 9990,
    secure: input.secure ?? false,
    apiKey: input.apiKey,
  };
}

/**
 * Bridge connection status
 */
export type ConnectionStatus = "disconnected" | "connecting" | "connected" | "error";

/**
 * Cached system info from /api/system (simplified for storage)
 */
export interface CachedSystemInfo {
  hostname?: string;
  os?: string;
  cpu?: {
    brand: string;
    cores: number;
    threads: number;
  };
  memory?: {
    total: number; // bytes
  };
  gpu?: {
    name: string;
    vram?: number;
  };
}

/**
 * Full stored bridge data with defined fields
 */
export interface StoredBridge {
  /** Unique identifier (UUID) */
  id: string;
  /** Connection configuration */
  config: BridgeConfig;
  /** Display name (required) */
  name: string;
  /** MAC address for Wake-on-LAN */
  mac?: string;
  /** Cached /api/system data */
  systemInfo?: CachedSystemInfo;
  /** User's extra custom fields */
  metadata?: Record<string, unknown>;
}

/**
 * Bridge with runtime state (extends stored data)
 */
export interface BridgeConnection extends StoredBridge {
  /** Current connection status */
  status: ConnectionStatus;
}

/**
 * Bridge error types
 */
export type BridgeError =
  | { type: "network"; message: string }
  | { type: "auth"; message: string }
  | { type: "feature_disabled"; feature: string }
  | { type: "unknown"; message: string };

/**
 * Connection mode for hooks that require WebSocket connection.
 *
 * - "auto" (default): Automatically connect if disconnected. Will reconnect on rerenders if disconnected.
 * - "passive": Don't initiate connection. Only use data if already connected.
 * - "eager": Connect once on mount. Won't reconnect if manually disconnected.
 */
export type ConnectionMode = "auto" | "passive" | "eager";

/**
 * Pluggable persistence layer for bridge storage.
 *
 * When provided to `BridgesProvider`, replaces the default
 * localStorage/Zustand persistence with a custom data layer
 * (SQLite, Postgres, REST API, etc.).
 */
export interface BridgePersistence {
  /** Load all bridges on mount (hydration) */
  load: () => Promise<StoredBridge[]>;
  /** Persist a new bridge. Must succeed before bridge is considered added. */
  onBridgeAdd: (bridge: StoredBridge) => Promise<void>;
  /** Remove a bridge from persistence. Must succeed before bridge is considered removed. */
  onBridgeRemove: (id: string) => Promise<void>;
  /** Update a bridge in persistence. Must succeed before bridge is considered updated. */
  onBridgeUpdate: (
    id: string,
    updates: Partial<Omit<StoredBridge, "id">>,
  ) => Promise<void>;
}
