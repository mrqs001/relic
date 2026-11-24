import type { Scope } from "./api";

export const timeWindows = { "1h": 3600000, "2h": 7200000, "4h": 14400000, "8h": 28800000, "24h": 86400000, "1w": 604800000 } as const;
export type TimeWindow = keyof typeof timeWindows;
export function isTimeWindow(value: string | null): value is TimeWindow {
  return value !== null && Object.hasOwn(timeWindows, value);
}
export function resolveTimeScope(scope: Scope, window: TimeWindow | null, now: number): Scope {
  return window ? { ...scope, from: new Date(now - timeWindows[window]).toISOString(), to: new Date(now).toISOString() } : scope;
}
export function shiftInterval(from: string, to: string, direction: -1 | 1) {
  const start = Date.parse(from), end = Date.parse(to);
  // Bounds are inclusive at millisecond precision. Adjacent steps neither skip nor repeat a message.
  const duration = end - start + 1;
  return { from: new Date(start + direction * duration).toISOString(), to: new Date(end + direction * duration).toISOString() };
}
