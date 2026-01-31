import { useQuery } from "@tanstack/react-query";
import { useEffect, useMemo, useRef } from "react";
import { useBridgesContext } from "../context/bridges-provider";
import type { ConnectionMode } from "../types";
import type { StreamPayload } from "../types/api";

interface UseSystemStatsOptions {
  /** Specific stats fields to subscribe to (e.g., ['cpu', 'memory']) */
  fields?: Array<"cpu" | "memory" | "gpu" | "disks" | "network">;
  /**
   * Connection behavior when bridge is disconnected.
   * - "auto" (default): Automatically connect if disconnected
   * - "passive": Don't initiate connection, only use data if connected
   * - "eager": Connect once on mount, won't reconnect if disconnected later
   */
  connectionMode?: ConnectionMode;
}

/**
 * Hook for real-time system stats via WebSocket subscription.
 *
 * Multiple components can use this hook with different fields - subscriptions
 * are reference-counted and accumulated (not replaced).
 *
 * @example
 * ```tsx
 * // Subscribe to all stats (auto-connects if needed)
 * const { data } = useSystemStats(bridgeId);
 *
 * // Subscribe to specific fields only (for widgets)
 * const { data } = useSystemStats(bridgeId, { fields: ['cpu'] });
 *
 * // Passive mode - don't trigger connection
 * const { data } = useSystemStats(bridgeId, { connectionMode: 'passive' });
 *
 * // Multiple widgets work independently:
 * // <CpuCard /> uses { fields: ['cpu'] }
 * // <MemoryCard /> uses { fields: ['memory'] }
 * // Both will receive their data without conflict
 * ```
 */
export function useSystemStats(bridgeId: string, options?: UseSystemStatsOptions) {
  const { bridges, getWsManager, _hookConnect: connect } = useBridgesContext();
  const bridge = bridges.get(bridgeId);
  const connectionMode = options?.connectionMode ?? "auto";
  const hasConnectedOnce = useRef(false);

  // Handle connection based on mode
  useEffect(() => {
    if (!bridge || connectionMode === "passive") return;

    const shouldConnect = bridge.status === "disconnected" || bridge.status === "error";

    if (connectionMode === "auto" && shouldConnect) {
      connect(bridgeId);
    } else if (connectionMode === "eager" && shouldConnect && !hasConnectedOnce.current) {
      hasConnectedOnce.current = true;
      connect(bridgeId);
    }
  }, [bridge, bridgeId, connectionMode, connect]);

  // Stabilize fields array to prevent unnecessary re-subscriptions
  const fieldsKey = useMemo(
    () => (options?.fields ? options.fields.sort().join(",") : "all"),
    [options?.fields],
  );

  // Subscribe to WebSocket topic on mount
  useEffect(() => {
    const wsManager = getWsManager(bridgeId);
    if (!wsManager || bridge?.status !== "connected") return;

    // Build topic list
    const topics =
      fieldsKey === "all" ? ["stats"] : fieldsKey.split(",").map((f) => `stats.${f}`);

    // Subscribe to all topics - WebSocketManager handles refcounting
    // so multiple components can subscribe to overlapping topics safely
    const unsubscribes = topics.map((topic) => wsManager.subscribe(topic));

    // Cleanup: unsubscribe all on unmount
    return () => {
      unsubscribes.forEach((unsub) => unsub());
    };
  }, [bridgeId, bridge?.status, getWsManager, fieldsKey]);

  return useQuery<StreamPayload | null>({
    queryKey: ["stats", bridgeId],
    queryFn: () => null, // Initial value, WebSocket fills it
    staleTime: Infinity, // WebSocket keeps it fresh
    gcTime: 5 * 60 * 1000, // Keep in cache 5 min after unmount
    enabled: !!bridge && bridge.status === "connected",
  });
}
