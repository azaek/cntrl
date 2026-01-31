import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useRef } from "react";
import { useBridgesContext } from "../context/bridges-provider";
import type { ConnectionMode } from "../types";
import type { MediaAction, MediaStatus } from "../types/api";

interface UseMediaOptions {
  /**
   * Connection behavior when bridge is disconnected.
   * - "auto" (default): Automatically connect if disconnected
   * - "passive": Don't initiate connection, only use data if connected
   * - "eager": Connect once on mount, won't reconnect if disconnected later
   */
  connectionMode?: ConnectionMode;
}

/**
 * Hook for media playback state and controls.
 *
 * @example
 * ```tsx
 * const { data, control } = useMedia(bridgeId);
 *
 * if (data) {
 *   console.log(`Now playing: ${data.title} by ${data.artist}`);
 *   console.log(`Volume: ${data.volume}%`);
 *   console.log(`Playing: ${data.playing}`);
 * }
 *
 * // Control playback
 * control.mutate({ action: 'play_pause' });
 * control.mutate({ action: 'set_volume', value: 50 });
 * control.mutate({ action: 'next' });
 * ```
 */
export function useMedia(bridgeId: string, options?: UseMediaOptions) {
  const queryClient = useQueryClient();
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

    return wsManager.subscribe("media");
  }, [bridgeId, bridge?.status, getWsManager]);

  const query = useQuery<MediaStatus | null>({
    queryKey: ["media", bridgeId],
    queryFn: () => null, // Initial value, WebSocket fills it
    staleTime: Infinity, // WebSocket keeps it fresh
    gcTime: 5 * 60 * 1000,
    enabled: !!bridge && bridge.status === "connected",
  });

  const control = useMutation({
    mutationFn: async (action: MediaAction) => {
      const wsManager = getWsManager(bridgeId);
      if (!wsManager) {
        throw new Error("Not connected to bridge");
      }

      wsManager.send({ op: "media", data: action });
    },
    onMutate: async (action) => {
      // Optimistic update for volume changes
      if (action.action === "set_volume" && action.value !== undefined) {
        await queryClient.cancelQueries({ queryKey: ["media", bridgeId] });
        const previous = queryClient.getQueryData<MediaStatus>(["media", bridgeId]);

        queryClient.setQueryData<MediaStatus | null>(["media", bridgeId], (old) =>
          old ? { ...old, volume: action.value! } : null,
        );

        return { previous };
      }

      // Optimistic update for mute toggles
      if (action.action === "toggle_mute") {
        await queryClient.cancelQueries({ queryKey: ["media", bridgeId] });
        const previous = queryClient.getQueryData<MediaStatus>(["media", bridgeId]);

        queryClient.setQueryData<MediaStatus | null>(["media", bridgeId], (old) =>
          old ? { ...old, muted: !old.muted } : null,
        );

        return { previous };
      }

      if (action.action === "mute") {
        await queryClient.cancelQueries({ queryKey: ["media", bridgeId] });
        const previous = queryClient.getQueryData<MediaStatus>(["media", bridgeId]);

        queryClient.setQueryData<MediaStatus | null>(["media", bridgeId], (old) =>
          old ? { ...old, muted: true } : null,
        );

        return { previous };
      }

      if (action.action === "unmute") {
        await queryClient.cancelQueries({ queryKey: ["media", bridgeId] });
        const previous = queryClient.getQueryData<MediaStatus>(["media", bridgeId]);

        queryClient.setQueryData<MediaStatus | null>(["media", bridgeId], (old) =>
          old ? { ...old, muted: false } : null,
        );

        return { previous };
      }
    },
    onError: (_err, _action, context) => {
      // Rollback on error
      if (context?.previous) {
        queryClient.setQueryData(["media", bridgeId], context.previous);
      }
    },
  });

  return { ...query, control };
}
