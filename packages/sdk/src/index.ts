/**
 * @cntrl-pw/sdk
 *
 * SDK for Cntrl Bridge devices - REST/WebSocket client and React hooks.
 */

// Store (Zustand - direct access for advanced usage)
export { useBridgesStore } from "./store/bridges-store";

// Context (runtime connections)
export { BridgesProvider, useBridgesContext } from "./context/bridges-provider";
export { useBridge, useBridges } from "./context/use-bridges";

// Hooks (all take bridgeId)
export {
  useBridgeStatus,
  useBridgesStatus,
  type BridgeStatusState,
} from "./hooks/use-bridge-status";
export { useMedia } from "./hooks/use-media";
export { sendWolPacket, usePower } from "./hooks/use-power";
export { useProcesses } from "./hooks/use-processes";
export { useSystemInfo } from "./hooks/use-system-info";
export { useSystemStats } from "./hooks/use-system-stats";

// Types
export * from "./types";
export * from "./types/api";
export * from "./types/ws";

// Client (low-level) - keep for direct usage
export { BridgeClient, createBridgeClient } from "./client";

// Utils
export { formatBytes, formatUptime } from "./utils";
