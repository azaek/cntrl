import { useQuery } from "@tanstack/react-query";
import { useBridgesContext } from "../context/bridges-provider";
import type { SystemInfo } from "../types/api";

/**
 * Hook for static system info via REST API.
 * This data rarely changes, so it's fetched once and cached.
 *
 * @example
 * ```tsx
 * const { data, isLoading } = useSystemInfo(bridgeId);
 *
 * if (data) {
 *   console.log(`Hostname: ${data.hostname}`);
 *   console.log(`CPU: ${data.cpu.brand}`);
 *   console.log(`RAM: ${formatBytes(data.memory.total)}`);
 * }
 * ```
 */
export function useSystemInfo(bridgeId: string) {
  const { bridges } = useBridgesContext();
  const bridge = bridges.get(bridgeId);

  return useQuery<SystemInfo | null>({
    queryKey: ["system-info", bridgeId],
    queryFn: async () => {
      if (!bridge) return null;

      const { config } = bridge;
      const protocol = config.secure ? "https" : "http";
      const baseUrl = `${protocol}://${config.host}:${config.port}`;

      const headers: HeadersInit = {};
      if (config.apiKey) {
        headers.Authorization = `Bearer ${config.apiKey}`;
      }

      const response = await fetch(`${baseUrl}/api/system`, { headers });

      if (!response.ok) {
        throw new Error(`Failed to fetch system info: ${response.statusText}`);
      }

      return response.json();
    },
    staleTime: 5 * 60 * 1000, // 5 minutes - rarely changes
    gcTime: 30 * 60 * 1000, // Keep in cache 30 min
    enabled: !!bridge,
  });
}
