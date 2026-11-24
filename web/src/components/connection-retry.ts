export function retrySeconds(until: string | null | undefined, now = Date.now()) {
  if (!until) return 0;
  const deadline = Date.parse(until);
  return Number.isFinite(deadline) ? Math.max(0, Math.ceil((deadline - now) / 1000)) : 0;
}

export function retryLabel(seconds: number) {
  if (seconds < 60) return `Retry in ${seconds}s`;
  const minutes = Math.ceil(seconds / 60);
  return minutes < 60 ? `Retry in ${minutes}m` : `Retry in ${Math.ceil(minutes / 60)}h`;
}
