import { useSyncExternalStore } from "react";
import { date } from "../api";
import { relativeAge } from "./reading-display";
let now = Date.now();
const listeners = new Set<() => void>();
let timer: ReturnType<typeof setInterval> | undefined;
const subscribe = (listener: () => void) => {
  listeners.add(listener);
  if (!timer) { now = Date.now(); timer = setInterval(() => { now = Date.now(); listeners.forEach(fn => fn()); }, 30000); }
  return () => { listeners.delete(listener); if (!listeners.size) { clearInterval(timer); timer = undefined; } };
};
/** The feed's time column: "12m", "3h", then the date. The full time is in
 *  the title and the accessible label. */
export function CompactTime({published_at, acquired_at}: {published_at?: string | null; acquired_at?: string}) {
  const clock = useSyncExternalStore(subscribe, () => now);
  const value = published_at || acquired_at;
  if (!value) return <span className="feed-time" aria-label="Time unknown">—</span>;
  const minutes = Math.floor((clock - Date.parse(value)) / 60000);
  const short = !Number.isFinite(minutes) ? "" : minutes < 1 ? "now" : minutes < 60 ? `${minutes}m` : minutes < 1440 ? `${Math.floor(minutes / 60)}h`
    : new Date(value).toLocaleDateString("en-GB", { day: "numeric", month: "short", timeZone: "UTC" });
  const full = `${published_at ? "Published" : "Received"} ${date(value)}, ${relativeAge(value, clock)}`;
  return <time className="feed-time" dateTime={value} title={full} aria-label={full}>{short}</time>;
}
export function MessageTime({published_at, acquired_at}: {published_at?: string | null; acquired_at?: string}) {
  const clock = useSyncExternalStore(subscribe, () => now);
  const value = published_at || acquired_at;
  if (!value) return null;
  return <time dateTime={value} title={`${published_at ? "Published" : "Received"} ${date(value)}`}>{!published_at && "Received "}{date(value, true)} · {relativeAge(value, clock)}</time>;
}
