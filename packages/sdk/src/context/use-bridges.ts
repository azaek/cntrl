import { useMemo } from "react";
import type { BridgeConnection } from "../types";
import { useBridgesContext } from "./bridges-provider";

/**
 * Hook to access all bridges and bridge management functions.
 *
 * @example
 * ```tsx
 * const { bridges, addBridge, removeBridge } = useBridges();
 *
 * // Add a new bridge
 * const id = addBridge({
 *   config: { host: '192.168.1.100', port: 9990 },
 *   name: 'Gaming PC',
 * });
 *
 * // List all bridges
 * bridges.forEach(bridge => {
 *   console.log(bridge.name, bridge.status);
 * });
 * ```
 */
export function useBridges() {
  const context = useBridgesContext();

  return useMemo(
    () => ({
      /** All bridges with runtime state */
      bridges: context.bridges,

      /** Add a new bridge and return its ID */
      addBridge: context.addBridge,

      /** Remove a bridge by ID */
      removeBridge: context.removeBridge,

      /** Update a bridge's configuration */
      updateBridge: context.updateBridge,

      /** Connect to a bridge */
      connect: context.connect,

      /** Disconnect from a bridge */
      disconnect: context.disconnect,

      /** Whether the store has been hydrated / loaded */
      ready: context.ready,

      /** Re-load bridges from persistence */
      refresh: context.refresh,

      /** Whether a custom persistence layer is in use (vs default localStorage) */
      isCustomStorage: context.isCustomStorage,

      /** Status of the initial data load ("idle" | "loading" | "error") */
      persistenceStatus: context.persistenceStatus,

      /** Error message from the initial load (null when no error) */
      persistenceError: context.persistenceError,
    }),
    [context],
  );
}

/**
 * Hook to access a single bridge by ID.
 *
 * @example
 * ```tsx
 * const bridge = useBridge('abc-123');
 *
 * if (!bridge) return <div>Bridge not found</div>;
 * if (bridge.status !== 'connected') return <div>Connecting...</div>;
 *
 * return <div>{bridge.name} - {bridge.systemInfo?.cpu?.brand}</div>;
 * ```
 */
export function useBridge(bridgeId: string): BridgeConnection | undefined {
  const { bridges } = useBridgesContext();
  return bridges.get(bridgeId);
}
