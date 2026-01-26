/**
 * Format uptime in seconds to human-readable string.
 *
 * @example
 * ```ts
 * formatUptime(60);        // "1m"
 * formatUptime(3600);      // "1h 0m"
 * formatUptime(86400);     // "1d 0h"
 * formatUptime(90061);     // "1d 1h"
 * formatUptime(0);         // "0s"
 * ```
 */
export function formatUptime(seconds: number): string {
  if (seconds < 60) {
    return `${Math.floor(seconds)}s`;
  }

  const days = Math.floor(seconds / 86400);
  const hours = Math.floor((seconds % 86400) / 3600);
  const minutes = Math.floor((seconds % 3600) / 60);

  if (days > 0) {
    return `${days}d ${hours}h`;
  }

  if (hours > 0) {
    return `${hours}h ${minutes}m`;
  }

  return `${minutes}m`;
}
