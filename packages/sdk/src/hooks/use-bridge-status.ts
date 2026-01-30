import { useQuery } from "@tanstack/react-query";
import { useBridgesContext } from "../context/bridges-provider";
import type { PingResult } from "../types/api";

interface UseBridgeStatusOptions {
  /** Polling interval in milliseconds (default: 30000) */
  interval?: number;
  /** Whether to enable polling (default: true) */
  enabled?: boolean;
}

/**
 * Status state for a bridge
 */
export type BridgeStatusState = "checking" | "online" | "offline" | "unknown";

/**
 * Hook to check bridge online status without establishing a WebSocket connection.
 * Uses polling to check /api/status endpoint.
 *
 * Useful for:
 * - Showing online/offline indicators in a bridge list
 * - Checking if a bridge is reachable before connecting
 * - Monitoring multiple bridges without full WS connections
 *
 * @example
 * ```tsx
 * function BridgeListItem({ bridgeId }: { bridgeId: string }) {
 *   const { state, online, responseTime, isChecking } = useBridgeStatus(bridgeId);
 *
 *   return (
 *     <div>
 *       <span>
 *         {state === 'checking' && '⏳'}
 *         {state === 'online' && '🟢'}
 *         {state === 'offline' && '🔴'}
 *         {state === 'unknown' && '⚪'}
 *       </span>
 *       {responseTime && <span>{responseTime}ms</span>}
 *     </div>
 *   );
 * }
 * ```
 */
export function useBridgeStatus(bridgeId: string, options?: UseBridgeStatusOptions) {
  const { bridges } = useBridgesContext();
  const bridge = bridges.get(bridgeId);
  const interval = options?.interval ?? 30000;
  const enabled = options?.enabled ?? true;

  const query = useQuery<PingResult>({
    queryKey: ["bridge-status", bridgeId],
    queryFn: async (): Promise<PingResult> => {
      if (!bridge) {
        return {
          online: false,
          responseTime: null,
          lastChecked: Date.now(),
        };
      }

      const { config } = bridge;
      const protocol = config.secure ? "https" : "http";
      const url = `${protocol}://${config.host}:${config.port}/api/status`;

      const start = performance.now();

      try {
        const controller = new AbortController();
        const timeoutId = setTimeout(() => controller.abort(), 5000);

        const response = await fetch(url, {
          signal: controller.signal,
        });

        clearTimeout(timeoutId);

        const responseTime = Math.round(performance.now() - start);

        return {
          online: response.ok,
          responseTime,
          lastChecked: Date.now(),
        };
      } catch {
        return {
          online: false,
          responseTime: null,
          lastChecked: Date.now(),
        };
      }
    },
    refetchInterval: enabled ? interval : false,
    enabled: enabled && !!bridge,
    staleTime: interval / 2,
  });

  // Derive status state
  const isChecking = query.isLoading || (query.isFetching && !query.data);
  const state: BridgeStatusState = isChecking
    ? "checking"
    : query.data?.online
      ? "online"
      : query.data
        ? "offline"
        : "unknown";

  return {
    // Convenience properties
    /** Current status: "checking" | "online" | "offline" | "unknown" */
    state,
    /** Whether bridge is online */
    online: query.data?.online ?? false,
    /** Response time in ms (null if offline/checking) */
    responseTime: query.data?.responseTime ?? null,
    /** Timestamp of last check */
    lastChecked: query.data?.lastChecked ?? null,
    /** Whether a check is in progress */
    isChecking,
    // Full query result for advanced usage
    ...query,
  };
}

/**
 * Hook to check multiple bridges' online status at once.
 *
 * @example
 * ```tsx
 * function BridgeSelector() {
 *   const { bridges } = useBridges();
 *   const bridgeIds = Array.from(bridges.keys());
 *   const { statuses, isChecking } = useBridgesStatus(bridgeIds, { interval: 15000 });
 *
 *   if (isChecking) return <div>Checking bridges...</div>;
 *
 *   return (
 *     <select>
 *       {bridgeIds.map((id) => (
 *         <option key={id} disabled={!statuses[id]?.online}>
 *           {bridges.get(id)?.name} {statuses[id]?.online ? '(online)' : '(offline)'}
 *         </option>
 *       ))}
 *     </select>
 *   );
 * }
 * ```
 */
export function useBridgesStatus(bridgeIds: string[], options?: UseBridgeStatusOptions) {
  const { bridges } = useBridgesContext();
  const interval = options?.interval ?? 30000;
  const enabled = options?.enabled ?? true;

  const query = useQuery<Record<string, PingResult>>({
    queryKey: ["bridges-status", bridgeIds.sort().join(",")],
    queryFn: async () => {
      const statuses: Record<string, PingResult> = {};

      await Promise.all(
        bridgeIds.map(async (bridgeId) => {
          const bridge = bridges.get(bridgeId);
          if (!bridge) {
            statuses[bridgeId] = {
              online: false,
              responseTime: null,
              lastChecked: Date.now(),
            };
            return;
          }

          const { config } = bridge;
          const protocol = config.secure ? "https" : "http";
          const url = `${protocol}://${config.host}:${config.port}/api/status`;

          const start = performance.now();

          try {
            const controller = new AbortController();
            const timeoutId = setTimeout(() => controller.abort(), 5000);

            const response = await fetch(url, {
              signal: controller.signal,
            });

            clearTimeout(timeoutId);

            statuses[bridgeId] = {
              online: response.ok,
              responseTime: Math.round(performance.now() - start),
              lastChecked: Date.now(),
            };
          } catch {
            statuses[bridgeId] = {
              online: false,
              responseTime: null,
              lastChecked: Date.now(),
            };
          }
        }),
      );

      return statuses;
    },
    refetchInterval: enabled ? interval : false,
    enabled: enabled && bridgeIds.length > 0,
    staleTime: interval / 2,
  });

  const isChecking = query.isLoading || (query.isFetching && !query.data);

  return {
    statuses: query.data ?? {},
    isChecking,
    // Full query for advanced usage
    ...query,
  };
}
