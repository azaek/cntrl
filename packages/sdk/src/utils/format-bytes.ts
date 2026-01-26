const UNITS = ["B", "KB", "MB", "GB", "TB", "PB"] as const;

/**
 * Format bytes to human-readable string.
 *
 * @example
 * ```ts
 * formatBytes(1024);        // "1.00 KB"
 * formatBytes(1536);        // "1.50 KB"
 * formatBytes(1073741824);  // "1.00 GB"
 * formatBytes(0);           // "0 B"
 * ```
 */
export function formatBytes(bytes: number, decimals = 2): string {
  if (bytes === 0) return "0 B";

  const k = 1024;
  const dm = decimals < 0 ? 0 : decimals;

  const i = Math.floor(Math.log(bytes) / Math.log(k));
  const unitIndex = Math.min(i, UNITS.length - 1);

  return `${parseFloat((bytes / Math.pow(k, unitIndex)).toFixed(dm))} ${UNITS[unitIndex]}`;
}
