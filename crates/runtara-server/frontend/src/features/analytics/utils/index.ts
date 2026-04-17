export function formatNumber(num: number): string {
  return num.toLocaleString();
}

export function formatDurationSeconds(seconds: number): string {
  if (seconds === 0) return '0s';
  if (seconds < 0.001) return `${(seconds * 1000000).toFixed(0)}µs`;
  if (seconds < 1) return `${(seconds * 1000).toFixed(2)}ms`;
  if (seconds < 60) return `${seconds.toFixed(2)}s`;
  if (seconds < 3600) {
    const minutes = Math.floor(seconds / 60);
    const secs = Math.floor(seconds % 60);
    return secs > 0 ? `${minutes}m ${secs}s` : `${minutes}m`;
  }
  const hours = Math.floor(seconds / 3600);
  const minutes = Math.floor((seconds % 3600) / 60);
  return minutes > 0 ? `${hours}h ${minutes}m` : `${hours}h`;
}

export function formatBytes(bytes: number): string {
  if (bytes === 0) return '0 B';
  const sizes = ['B', 'KB', 'MB', 'GB', 'TB'];
  const i = Math.floor(Math.log(bytes) / Math.log(1024));
  return `${(bytes / Math.pow(1024, i)).toFixed(2)} ${sizes[i]}`;
}

export function formatMemory(mb: number): string {
  if (mb === 0) return '0 MB';
  if (mb < 1) return `${(mb * 1024).toFixed(1)} KB`;
  if (mb < 1024) return `${mb.toFixed(1)} MB`;
  return `${(mb / 1024).toFixed(2)} GB`;
}

export function calculatePercentageChange(
  current: number,
  previous: number
): number {
  if (previous === 0) return current > 0 ? 100 : 0;
  return ((current - previous) / previous) * 100;
}

export function determineTrend(
  current: number,
  previous: number,
  threshold: number = 5
): 'up' | 'down' | 'stable' {
  const change = calculatePercentageChange(current, previous);
  if (Math.abs(change) < threshold) return 'stable';
  return change > 0 ? 'up' : 'down';
}
