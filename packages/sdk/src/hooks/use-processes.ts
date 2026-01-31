import { useMutation, useQuery } from "@tanstack/react-query";
import { useEffect, useRef } from "react";
import { useBridgesContext } from "../context/bridges-provider";
import type { ConnectionMode } from "../types";
import type { ProcessListPayload } from "../types/api";

interface UseProcessesOptions {
  /**
   * Connection behavior when bridge is disconnected.
   * - "auto" (default): Automatically connect if disconnected
   * - "passive": Don't initiate connection, only use data if connected
   * - "eager": Connect once on mount, won't reconnect if disconnected later
   */
  connectionMode?: ConnectionMode;
}

/**
 * Hook for running processes list and controls.
 *
 * @example
 * ```tsx
 * const { data, kill, launch } = useProcesses(bridgeId);
 *
 * // List processes (aggregated by name)
 * data?.processes.forEach(proc => {
 *   console.log(`${proc.name}: ${proc.memory_mb}MB (${proc.count} instances)`);
 * });
 *
 * // Kill a process by PID
 * kill.mutate({ pid: 1234 });
 *
 * // Kill all processes by name
 * kill.mutate({ name: 'notepad.exe' });
 *
 * // Launch an application
 * launch.mutate({ path: 'notepad.exe' });
 * ```
 */
export function useProcesses(bridgeId: string, options?: UseProcessesOptions) {
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

  // Subscribe to WebSocket topic on mount
  useEffect(() => {
    const wsManager = getWsManager(bridgeId);
    if (!wsManager || bridge?.status !== "connected") return;

    return wsManager.subscribe("processes");
  }, [bridgeId, bridge?.status, getWsManager]);

  const query = useQuery<ProcessListPayload | null>({
    queryKey: ["processes", bridgeId],
    queryFn: () => null, // Initial value, WebSocket fills it
    staleTime: Infinity, // WebSocket keeps it fresh
    gcTime: 5 * 60 * 1000,
    enabled: !!bridge && bridge.status === "connected",
  });

  const kill = useMutation({
    mutationFn: async (params: { pid?: number; name?: string }) => {
      const wsManager = getWsManager(bridgeId);
      if (!wsManager) {
        throw new Error("Not connected to bridge");
      }

      wsManager.send({ op: "process_kill", data: params });
    },
  });

  const launch = useMutation({
    mutationFn: async (params: { path: string; args?: string[] }) => {
      const wsManager = getWsManager(bridgeId);
      if (!wsManager) {
        throw new Error("Not connected to bridge");
      }

      wsManager.send({
        op: "process_launch",
        data: { path: params.path, args: params.args },
      });
    },
  });

  const focus = useMutation({
    mutationFn: async (pid: number) => {
      const wsManager = getWsManager(bridgeId);
      if (!wsManager) {
        throw new Error("Not connected to bridge");
      }

      wsManager.send({ op: "process_focus", data: { pid } });
    },
  });

  return { ...query, kill, launch, focus };
}
