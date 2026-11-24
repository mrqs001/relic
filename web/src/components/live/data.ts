import type { Feature, FeatureCollection, Point } from "geojson";
import { z } from "zod";
import { api } from "../../api";
import { movement, silhouette, SILHOUETTE_SCALE } from "./symbols";

export const empty: FeatureCollection = { type: "FeatureCollection", features: [] };

export const aircraftViewSchema = z.object({
  features: z.array(z.any()).default([]),
  meta: z.record(z.string(), z.unknown()).nullish(),
  acquired_at: z.string().nullish(), stale: z.boolean().nullish(), error: z.string().nullish(),
}).passthrough();

export type Props = Record<string, unknown>;

/** A vessel's current detail, shared by hover and card: KeelWatch's record,
 *  or MarineTraffic's for a MarineTraffic ship (keys `mt:<ship id>`). */
export const shipDetail = (key: string) => ({
  queryKey: ["live-ship", key],
  staleTime: 60_000,
  queryFn: ({ signal }: { signal: AbortSignal }) => api(key.startsWith("mt:") ? `/layers/ships/marinetraffic/${encodeURIComponent(key.slice(3))}` : `/layers/ships/${encodeURIComponent(key)}`, z.object({ vessel: z.record(z.string(), z.any()).nullish(), error: z.string().nullish() }).passthrough(), { signal }),
});

/** A KeelWatch tile feature as card and tooltip properties. Tiles up to zoom
 *  5 carry a light property set and may stand for `n` ships; from zoom 6 every
 *  ship is its own feature with full properties. The feature id is the MMSI.
 *  MarineTraffic features use the same names, with no feature id: their key
 *  is `mt:<MarineTraffic ship id>` (or `mt:sat:…` for a withheld contact). */
export function shipProps(featureId: unknown, raw: Props, at?: [number, number]): Props {
  const mmsi = raw.mmsi ?? (featureId != null && featureId !== "" ? featureId : null);
  const sog = raw.sog == null ? null : Number(raw.sog);
  const move = movement(raw.nav as string | null, sog);
  const heading = raw.hdg != null && Number(raw.hdg) < 360 ? Number(raw.hdg) : null;
  return {
    key: raw.id ? String(raw.id) : mmsi != null ? `mmsi:${mmsi}` : null,
    id: raw.id ?? null, mmsi, imo: raw.imo ?? null, category: raw.cat ?? "other", sog, cog: raw.cog ?? raw.dir ?? null, heading,
    name: raw.name ?? null, type: raw.type ?? null, flag: raw.flag ?? null, callsign: raw.callsign ?? null, destination: raw.dest ?? null,
    nav_status: raw.nav ?? null, age_min: raw.age ?? null, sat: raw.sat === true || raw.sat === 1, time_estimated: raw.est === true || raw.est === 1,
    length_m: raw.len ?? null, width_m: raw.wid ?? null, identity_status: raw.identity ?? null, source: raw.src ?? null,
    // MarineTraffic features: which source drew the ship, and its own id.
    provider: raw.provider ?? null, ship_id: raw.ship_id ?? null, dwt: raw.dwt ?? null,
    // Fix time, epoch seconds (full tile properties only).
    observed_at: raw.ts != null && Number.isFinite(Number(raw.ts)) ? new Date(Number(raw.ts) * 1000).toISOString() : null,
    n: Number(raw.n ?? 1), movement: move, moving: move === "moving",
    ...(at ? { lon: at[0], lat: at[1] } : {}),
  };
}

/** Aircraft features with silhouette, size and colour tone. */
export function aircraftFeatures(features: Feature[]): Feature[] {
  return features.map(f => {
    const p = { ...(f.properties ?? {}) } as Props;
    const [lon, lat] = (f.geometry as Point).coordinates;
    p.lon = lon; p.lat = lat;
    const shape = silhouette(Number(p.icon ?? 0), p.aircraft_type as string | null);
    p.shape = shape; p.scale = SILHOUETTE_SCALE[shape];
    p.tone = p.military === true ? "military" : shape === "ground" ? "ground" : "civil";
    return { ...f, properties: p };
  });
}

export function ago(value: string | number | null | undefined) {
  if (value === null || value === undefined || value === "") return "time unknown";
  const ms = typeof value === "number" ? value : Date.parse(value);
  if (!Number.isFinite(ms)) return "time unknown";
  const s = Math.max(0, (Date.now() - ms) / 1000);
  return s < 90 ? `${Math.round(s)} s ago` : s < 5400 ? `${Math.round(s / 60)} min ago` : s < 172800 ? `${Math.round(s / 3600)} h ago` : `${Math.round(s / 86400)} d ago`;
}
export const utc = (ms: number) => new Date(ms).toISOString().slice(0, 16).replace("T", " ") + " UTC";
/** The age of a vessel report from KeelWatch minutes, relative to fetch time. */
export const reportAge = (minutes: unknown, fetched: string | null) => {
  const m = Number(minutes);
  if (!Number.isFinite(m)) return "time unknown";
  const base = fetched ? Date.parse(fetched) : Date.now();
  return ago(base - m * 60_000);
};
export const altitude = (ft: unknown, ground?: unknown) => ground ? "On ground" : Number(ft) >= 18_000 ? `FL${Math.round(Number(ft) / 100)}` : `${Number(ft).toLocaleString("en-GB")} ft`;
export const text = (v: unknown) => (typeof v === "string" && v.trim()) ? v.trim() : null;
/** A number only when one was given: null, "" and absent values never become 0. */
export const finite = (v: unknown): number | null => {
  const n = typeof v === "number" ? v : typeof v === "string" && v.trim() ? Number(v) : NaN;
  return Number.isFinite(n) ? n : null;
};
/** A longitude and latitude pair, or null when either is missing or out of range. */
export const position = (lon: unknown, lat: unknown): [number, number] | null => {
  const x = finite(lon), y = finite(lat);
  return x != null && y != null && Math.abs(x) <= 180 && Math.abs(y) <= 90 ? [x, y] : null;
};
/** Whether a longitude lies in a map view whose edges may run past ±180. */
export const inView = (bounds: { getWest(): number; getEast(): number; getSouth(): number; getNorth(): number }, [lon, lat]: [number, number]) => {
  const west = bounds.getWest(), east = bounds.getEast();
  if (lat < bounds.getSouth() || lat > bounds.getNorth()) return false;
  return east - west >= 360 || (((lon - west) % 360) + 360) % 360 <= east - west;
};

export type Sample = { t: number; lon: number; lat: number; v: number | null };
/** A path as two-point segments, each carrying the mean value of its ends
 *  (altitude or speed) for colouring; joined only across plausible steps. */
export function valueSegments(points: Sample[], maxGapS: number, maxKnots: number, withFixes = true): Feature[] {
  const out: Feature[] = [];
  const sorted = [...points].sort((a, b) => a.t - b.t);
  sorted.forEach((p, i) => {
    if (withFixes) out.push({ type: "Feature", properties: p.v == null ? {} : { v: p.v }, geometry: { type: "Point", coordinates: [p.lon, p.lat] } });
    const q = sorted[i - 1];
    if (!q) return;
    const dt = p.t - q.t;
    const km = Math.hypot((p.lon - q.lon) * 111 * Math.cos(p.lat * Math.PI / 180), (p.lat - q.lat) * 111);
    const knots = dt > 0 ? km / 1.852 / (dt / 3600) : Infinity;
    if (dt > maxGapS || knots > maxKnots || Math.abs(p.lon - q.lon) > 180) return;
    const v = p.v != null && q.v != null ? (p.v + q.v) / 2 : p.v ?? q.v;
    out.push({ type: "Feature", properties: v == null ? {} : { v }, geometry: { type: "LineString", coordinates: [[q.lon, q.lat], [p.lon, p.lat]] } });
  });
  return out;
}

/** Actual fixes joined only across plausible steps; gaps stay gaps. */
export function trackLines(points: { t: number; lon: number; lat: number }[], maxGapS: number, maxKnots: number, withFixes = true): Feature[] {
  const out: Feature[] = [];
  let line: [number, number][] = [];
  const flush = () => { if (line.length > 1) out.push({ type: "Feature", properties: {}, geometry: { type: "LineString", coordinates: line } }); line = []; };
  const sorted = [...points].sort((a, b) => a.t - b.t);
  sorted.forEach((p, i) => {
    if (withFixes) out.push({ type: "Feature", properties: {}, geometry: { type: "Point", coordinates: [p.lon, p.lat] } });
    const q = sorted[i - 1];
    if (q) {
      const dt = p.t - q.t;
      const km = Math.hypot((p.lon - q.lon) * 111 * Math.cos(p.lat * Math.PI / 180), (p.lat - q.lat) * 111);
      const knots = dt > 0 ? km / 1.852 / (dt / 3600) : Infinity;
      if (dt > maxGapS || knots > maxKnots || Math.abs(p.lon - q.lon) > 180) flush();
    }
    line.push([p.lon, p.lat]);
  });
  flush();
  return out;
}
