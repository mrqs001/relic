import { useCallback, useEffect, useRef, useState, type MutableRefObject } from "react";
import maplibregl from "maplibre-gl";
import type { Feature, Point } from "geojson";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { z } from "zod";
import { Layers } from "lucide-react";
import { api } from "../api";
import { ALTITUDE_RAMP, AIRCRAFT_COLOR, AIRCRAFT_GROUPS, ICON_PREFIX, SHIP_CATEGORIES, SPEED_RAMP, installIcons, rampColor, shipColor, type Ramp, type ShipCategory } from "./live/symbols";
import { aircraftFeatures, aircraftViewSchema, empty, finite, inView, position, shipProps, valueSegments, type Props, type Sample } from "./live/data";
import LivePanel, { defaults, idle, type LayerHealth, type Pick, type Settings, type Status } from "./live/LivePanel";
import LiveCard, { type Selected, type Track } from "./live/LiveCard";
import LiveTooltip, { type Hover } from "./live/LiveTooltip";
import AircraftExtras from "./live/AircraftExtras";
import { AircraftHistory } from "./live/aircraft";
import { HOT_WARNINGS, WARNING_COLOR, WARNING_OTHER, fromDays } from "./live/Warnings";
import "../live-layers.css";
import "../live-aircraft.css";

/** Context layers: ships (KeelWatch, or MarineTraffic's live map), Flightradar24
 *  aircraft and MSCIO warnings. Live by default. At a past moment (chosen here,
 *  or a historical snapshot's time) ships come from KeelWatch's stored history,
 *  warnings are those registered by then, and aircraft and MarineTraffic ships
 *  are hidden: Relic keeps no aircraft or MarineTraffic history. */
const STORE = "relic.live-layers.v2";
function loadSettings(): Settings {
  try {
    const saved = JSON.parse(localStorage.getItem(STORE) ?? "null") ?? JSON.parse(localStorage.getItem("relic.live-layers") ?? "{}");
    // Warning windows were whole days before 26 October 2026.
    const warningHours = typeof saved.warningHours === "string" ? saved.warningHours : fromDays(saved.warningDays) ?? defaults.warningHours;
    const { warningDays: _days, ...rest } = saved;
    return { ...defaults, ...rest, warningHours, shipSource: saved.shipSource === "marinetraffic" ? "marinetraffic" : "keelwatch", shipCategories: Array.isArray(saved.shipCategories) ? saved.shipCategories : defaults.shipCategories, aircraftGroups: Array.isArray(saved.aircraftGroups) ? saved.aircraftGroups : defaults.aircraftGroups };
  } catch { return defaults; }
}
const warningColor = ["match", ["get", "type"], ...Object.entries(WARNING_COLOR).flat(), WARNING_OTHER] as unknown as maplibregl.ExpressionSpecification;
/** Attacks, hijacks and kidnaps draw a little larger, as in Recall. */
const warningScale: maplibregl.ExpressionSpecification = ["case", ["in", ["get", "type"], ["literal", HOT_WARNINGS]], 1.18, 1];
/** Listed unless MSCIO's list was judged to leave it out. Unlisted warnings
 *  are hollow (outline in the type's colour) and dimmer: no longer current,
 *  but not cancelled either. */
const listed: maplibregl.ExpressionSpecification = ["!=", ["get", "listed"], false];
const warningAge: maplibregl.ExpressionSpecification = ["interpolate", ["linear"], ["get", "age_days"], 0, 0.95, 7, 0.95, 30, 0.5, 120, 0.25];
/** Provider health per layer, for the Layers panel. */
const layersSchema = z.object({ layers: z.array(z.object({ id: z.string(), health: z.object({ state: z.string(), detail: z.string().nullish(), since: z.string().nullish(), checked_at: z.string().nullish() }).nullish() }).passthrough()) });
/** Layers the pointer can hover and click, top first. */
const INTERACTIVE = ["live-aircraft", "live-ships", "live-ship-dots", "live-mt-ships", "live-mt-ship-dots", "live-warnings"];
const SHIP_LAYERS = ["live-ship-dots", "live-ships", "live-ship-names"];
/** MarineTraffic ships: the same symbols from a GeoJSON source of the view. */
const MT_LAYERS = ["live-mt-ship-dots", "live-mt-ships", "live-mt-ship-names"];
/** MarineTraffic tiles are cached by Relic for two minutes; the view refreshes
 *  every minute, then after failures waits 1, 2, then 4 minutes. */
const MT_REFRESH_MS = 60_000;
const MT_BACKOFF_MS = [60_000, 120_000, 240_000];
const mtSchema = z.object({ features: z.array(z.any()).default([]), meta: z.record(z.string(), z.unknown()).nullish(), acquired_at: z.string().nullish(), stale: z.boolean().nullish(), incomplete: z.boolean().nullish(), error: z.string().nullish() }).passthrough();
/** Each vessel's last fix in this window up to a past moment. */
const PAST_WINDOW = "2h";
const coverageSchema = z.object({ gaps: z.array(z.object({ from: z.string(), to: z.string() })).default([]) }).passthrough();
/** Moments snap to ten minutes: every distinct minute is a new tile set at
 *  KeelWatch, and its sources report every 2 to 15 minutes. */
export const snapMoment = (iso: string) => new Date(Math.min(Date.now(), Math.floor(Date.parse(iso) / 600_000) * 600_000)).toISOString().replace(".000Z", "Z");
const when = (iso: string) => new Date(iso).toLocaleString("en-GB", { day: "numeric", month: "short", hour: "2-digit", minute: "2-digit", timeZone: "UTC" });
/** Colour each path segment and fix by its value on the measure's ramp. */
const coloured = (features: Feature[], ramp: Ramp) => features.map(f => ({ ...f, properties: { ...f.properties, c: f.properties?.v != null ? rampColor(ramp, Number(f.properties.v)) : "#ffffff" } }));
const REPORT_LAYERS = ["evidence-points", "evidence-clusters"];
/** KeelWatch vector tiles through Relic; `v` changes to fetch fresh tiles. */
const tileUrl = (freshness: string, version: number, past: string | null = null) => past
  ? `${location.origin}/api/v1/layers/ships/tiles/{z}/{x}/{y}?at=${encodeURIComponent(past)}&window=${PAST_WINDOW}`
  : `${location.origin}/api/v1/layers/ships/tiles/{z}/{x}/{y}?freshness=${freshness}&v=${version}`;
const SHIP_REFRESH_MS = 30_000;
/** After failed aircraft refreshes the next tries wait this long, then stay at the last. */
const AIRCRAFT_BACKOFF_MS = [30_000, 60_000, 120_000];
/** A followed target this close to the centre (in pixels) is left where it is. */
const FOLLOW_SLACK_PX = 4;
/** Fields a provider search result carries from now, not from a past moment. */
const CURRENT_ONLY = new Set(["lon", "lat", "sog", "cog", "heading", "nav_status", "destination", "observed_at", "age_min", "movement", "moving"]);
/** Camera moves this code starts carry this flag, so they trigger no refresh. */
type Own = { liveFollow?: boolean };
const own = (e: unknown) => !!(e as Own | undefined)?.liveFollow;
/** A failed KeelWatch tile, in words; Relic's JSON error message when it sent one. */
async function tileFailure(error: unknown) {
  const e = error as { status?: number; body?: unknown };
  let detail = e.status ? `HTTP ${e.status}` : "no response";
  if (e.body instanceof Blob) {
    try { const message = (JSON.parse(await e.body.text()) as { error?: { message?: unknown } }).error?.message; if (typeof message === "string" && message) detail = message; } catch { /* the status says enough */ }
  }
  return `Ship tiles failed (${detail}). Ships here were not received, not absent.`;
}

/** The live feature nearest the pointer (within 7 px, so small dots are easy
 *  to hit), unless a report marker is under it: reports always win. */
export function liveFeatureNear(m: maplibregl.Map, point: maplibregl.Point): maplibregl.MapGeoJSONFeature | null {
  const layers = INTERACTIVE.filter(id => m.getLayer(id) && m.getLayoutProperty(id, "visibility") !== "none");
  if (!layers.length) return null;
  const reports = REPORT_LAYERS.filter(id => m.getLayer(id));
  if (reports.length && m.queryRenderedFeatures(point, { layers: reports }).length) return null;
  const box: [maplibregl.PointLike, maplibregl.PointLike] = [[point.x - 7, point.y - 7], [point.x + 7, point.y + 7]];
  let best: maplibregl.MapGeoJSONFeature | null = null, distance = Infinity;
  for (const h of m.queryRenderedFeatures(box, { layers })) {
    const c = m.project((h.geometry as Point).coordinates as [number, number]);
    const d = Math.hypot(c.x - point.x, c.y - point.y) + INTERACTIVE.indexOf(h.layer.id) * 0.5;
    if (d < distance) { distance = d; best = h; }
  }
  return best;
}

const moving: maplibregl.ExpressionSpecification = [">=", ["coalesce", ["get", "sog"], 0], 0.5];
/** A report fades with its age in minutes (`age`) rather than vanishing: an
 *  anchorage of ships last heard hours ago still shows, quieter. */
const reportAge = (opacity: number): maplibregl.ExpressionSpecification => ["interpolate", ["linear"], ["coalesce", ["get", "age"], 0], 30, opacity, 120, opacity * 0.82, 360, opacity * 0.64, 1440, opacity * 0.42];
/** Category and movement filters, applied by the map without refetching. */
function shipFilter(st: Settings): maplibregl.FilterSpecification | null {
  const parts: maplibregl.ExpressionSpecification[] = [];
  if (st.shipCategories.length < SHIP_CATEGORIES.length) parts.push(["in", ["coalesce", ["get", "cat"], "other"], ["literal", st.shipCategories]]);
  if (st.movement === "moving") parts.push(moving);
  if (st.movement === "still") parts.push(["!", moving]);
  return parts.length ? ["all", ...parts] as maplibregl.FilterSpecification : null;
}

function install(m: maplibregl.Map, st: Settings, version: number, past: string | null) {
  installIcons(m);
  const before = m.getLayer("area-fill") ? "area-fill" : undefined;
  for (const id of ["live-aircraft", "live-track", "live-warnings", "live-selected", "live-mt-ships"]) if (!m.getSource(id)) m.addSource(id, { type: "geojson", data: empty });
  if (!m.getSource("live-ships")) m.addSource("live-ships", { type: "vector", tiles: [tileUrl(st.freshness, version, past)], minzoom: 0, maxzoom: 14 });
  const glyphs = !!m.getStyle().glyphs;
  const add = (layer: maplibregl.LayerSpecification) => { if (!m.getLayer(layer.id)) m.addLayer(layer, before); };
  const vessels = { source: "live-ships", "source-layer": "vessels" } as const;
  // The selected path: a dark casing, then segments coloured by altitude or speed.
  const pathColor: maplibregl.ExpressionSpecification = ["coalesce", ["get", "c"], "#ffffff"];
  add({ id: "live-track-casing", type: "line", source: "live-track", filter: ["==", ["geometry-type"], "LineString"], layout: { "line-cap": "round", "line-join": "round" }, paint: { "line-color": "#04101a", "line-width": ["interpolate", ["linear"], ["zoom"], 3, 3.4, 10, 5.4], "line-opacity": 0.75 } });
  add({ id: "live-track-line", type: "line", source: "live-track", filter: ["==", ["geometry-type"], "LineString"], layout: { "line-cap": "round", "line-join": "round" }, paint: { "line-color": pathColor, "line-width": ["interpolate", ["linear"], ["zoom"], 3, 1.6, 10, 3], "line-opacity": 0.95 } });
  add({ id: "live-track-fixes", type: "circle", source: "live-track", filter: ["==", ["geometry-type"], "Point"], minzoom: 6, paint: { "circle-radius": ["interpolate", ["linear"], ["zoom"], 6, 1.6, 12, 3], "circle-color": pathColor, "circle-stroke-color": "#04101a", "circle-stroke-width": 0.8 } });
  // Zoomed out: one dot per ship (or per half pixel of ships, a little larger).
  const many: maplibregl.ExpressionSpecification = ["*", 0.4, ["ln", ["coalesce", ["get", "n"], 1]]];
  // Zoomed in: hull shapes along the heading, sized by reported length; round
  // when stopped or when no direction was reported.
  const length: maplibregl.ExpressionSpecification = ["interpolate", ["linear"], ["coalesce", ["get", "len"], 60], 8, 0.72, 60, 1, 200, 1.3, 400, 1.6];
  const pointed: maplibregl.ExpressionSpecification = ["all", moving, ["any", ["has", "hdg"], ["has", "cog"], ["has", "dir"]]];
  const hull = (a: number, b: number): maplibregl.ExpressionSpecification => ["*", length, ["case", pointed, a, b]];
  const ships = (source: { source: string; "source-layer"?: string }, dots: string, hulls: string, opacity: number) => {
    add({ id: dots, type: "circle", ...source, maxzoom: 6.5, paint: { "circle-color": shipColor, "circle-radius": ["interpolate", ["linear"], ["zoom"], 0, ["+", 0.9, many], 3, ["+", 1.5, many], 6.5, ["+", 3, many]], "circle-stroke-color": "#06111a", "circle-stroke-width": ["interpolate", ["linear"], ["zoom"], 1, 0, 4, 0.6], "circle-opacity": reportAge(opacity) } } as unknown as maplibregl.LayerSpecification);
    add({ id: hulls, type: "symbol", ...source, minzoom: 6.5, layout: {
      "icon-image": ["case", pointed, `${ICON_PREFIX}ship`, `${ICON_PREFIX}ship-still`],
      "icon-size": ["interpolate", ["linear"], ["zoom"], 6.5, hull(0.34, 0.23), 9, hull(0.48, 0.29), 13, hull(0.88, 0.44)],
      "icon-rotate": ["case", ["<", ["coalesce", ["get", "hdg"], 999], 360], ["get", "hdg"], ["coalesce", ["get", "cog"], ["get", "dir"], 0]],
      "icon-rotation-alignment": "map", "icon-allow-overlap": true, "icon-ignore-placement": true },
      paint: { "icon-color": shipColor, "icon-halo-color": "#06111a", "icon-halo-width": 1.3, "icon-opacity": reportAge(opacity) } } as unknown as maplibregl.LayerSpecification);
  };
  ships(vessels, "live-ship-dots", "live-ships", 0.96);
  ships({ source: "live-mt-ships" }, "live-mt-ship-dots", "live-mt-ships", 0.96);
  add({ id: "live-warnings-new", type: "circle", source: "live-warnings", filter: ["all", listed, ["<", ["get", "age_days"], 1]], paint: { "circle-radius": ["interpolate", ["linear"], ["zoom"], 2, 9, 8, 15], "circle-color": "transparent", "circle-stroke-color": warningColor, "circle-stroke-width": 1.4, "circle-stroke-opacity": 0.55, "circle-blur": 0.15 } });
  add({ id: "live-warnings", type: "symbol", source: "live-warnings", layout: { "icon-image": `${ICON_PREFIX}warning`, "icon-size": ["interpolate", ["linear"], ["zoom"], 2, ["*", 0.42, warningScale], 8, ["*", 0.62, warningScale]], "icon-allow-overlap": true, "icon-ignore-placement": true },
    paint: { "icon-color": ["case", listed, warningColor, "rgba(11,21,32,0.55)"], "icon-opacity": ["*", ["case", listed, 1, 0.6], warningAge], "icon-halo-color": ["case", listed, "#0b1520", warningColor], "icon-halo-width": ["case", listed, 1, 1.6] } });
  // Aircraft: a short trail through recent reports, fading towards its tail,
  // and a red ring round any aircraft whose feed status is an emergency.
  if (!m.getSource("live-aircraft-trails")) m.addSource("live-aircraft-trails", { type: "geojson", data: empty, lineMetrics: true });
  const trail = (tone: string, color: string) => add({ id: `live-aircraft-trail-${tone}`, type: "line", source: "live-aircraft-trails", minzoom: 4, filter: ["==", ["get", "tone"], tone], layout: { "line-cap": "round", "line-join": "round" },
    paint: { "line-width": ["interpolate", ["linear"], ["zoom"], 4, 1, 9, 1.8, 13, 2.6], "line-gradient": ["interpolate", ["linear"], ["line-progress"], 0, `${color}00`, 1, `${color}b3`] } });
  trail("civil", AIRCRAFT_COLOR.civil); trail("military", AIRCRAFT_COLOR.military);
  add({ id: "live-aircraft-alert", type: "circle", source: "live-aircraft", filter: ["==", ["get", "emergency"], true], paint: {
    "circle-radius": ["interpolate", ["linear"], ["zoom"], 2, 12, 8, 21, 13, 29], "circle-color": "#ff3b3b", "circle-opacity": 0.16,
    "circle-stroke-color": "#ff5a5a", "circle-stroke-width": 2, "circle-stroke-opacity": 0.95, "circle-stroke-opacity-transition": { duration: 800, delay: 0 }, "circle-opacity-transition": { duration: 800, delay: 0 } } });
  add({ id: "live-aircraft", type: "symbol", source: "live-aircraft", layout: {
    "icon-image": ["concat", ICON_PREFIX, ["get", "shape"]],
    // Zoom must lead the expression; each stop scales by the silhouette's size.
    "icon-size": ["interpolate", ["linear"], ["zoom"], 2, ["*", ["get", "scale"], 0.4], 6, ["*", ["get", "scale"], 0.56], 10, ["*", ["get", "scale"], 0.85], 14, ["*", ["get", "scale"], 1.1]],
    "icon-rotate": ["to-number", ["get", "track"], 0], "icon-rotation-alignment": "map", "icon-allow-overlap": true, "icon-ignore-placement": true,
    // Emergencies, then military and government aircraft, draw above other traffic.
    "symbol-sort-key": ["case", ["==", ["get", "emergency"], true], 2, ["==", ["get", "tone"], "military"], 1, 0] },
    paint: { "icon-color": ["match", ["get", "tone"], "military", AIRCRAFT_COLOR.military, "ground", AIRCRAFT_COLOR.ground, AIRCRAFT_COLOR.civil], "icon-halo-color": "#07121b", "icon-halo-width": 1.3, "icon-opacity": ["case", ["get", "on_ground"], 0.62, 1] } });
  if (glyphs) {
    const label: maplibregl.SymbolLayerSpecification["layout"] = { "text-font": ["Noto Sans Regular"], "text-size": 10.5, "text-offset": [0, 1.35], "text-anchor": "top", "text-optional": true, "text-max-width": 12 };
    add({ id: "live-ship-names", type: "symbol", ...vessels, minzoom: 10.5, layout: { ...label, "text-field": ["coalesce", ["get", "name"], ""] }, paint: { "text-color": "#d7e5ec", "text-halo-color": "#06111a", "text-halo-width": 1.4 } });
    add({ id: "live-mt-ship-names", type: "symbol", source: "live-mt-ships", minzoom: 10.5, layout: { ...label, "text-field": ["coalesce", ["get", "name"], ""] }, paint: { "text-color": "#d7e5ec", "text-halo-color": "#06111a", "text-halo-width": 1.4 } });
    add({ id: "live-aircraft-labels", type: "symbol", source: "live-aircraft", minzoom: 7.5, filter: ["!", ["get", "on_ground"]], layout: { ...label, "text-field": ["coalesce", ["get", "callsign"], ""] }, paint: { "text-color": ["case", ["==", ["get", "emergency"], true], "#ffc2c2", ["==", ["get", "tone"], "military"], "#ffc9c4", "#f3e3b4"], "text-halo-color": "#07121b", "text-halo-width": 1.4 } });
  }
  add({ id: "live-selected", type: "circle", source: "live-selected", paint: { "circle-radius": ["interpolate", ["linear"], ["zoom"], 2, 10, 10, 16], "circle-color": "transparent", "circle-stroke-color": "#ffffff", "circle-stroke-width": 2, "circle-stroke-opacity": 0.95 } });
}
/** Which ship layers show: the chosen source's, never both; MarineTraffic only live. */
function shipVisibility(m: maplibregl.Map, st: Settings, past: string | null) {
  const mt = st.shipSource === "marinetraffic";
  for (const id of SHIP_LAYERS) if (m.getLayer(id)) m.setLayoutProperty(id, "visibility", st.ships && !mt ? "visible" : "none");
  for (const id of MT_LAYERS) if (m.getLayer(id)) m.setLayoutProperty(id, "visibility", st.ships && mt && !past ? "visible" : "none");
}
/** MarineTraffic ships from a failed refresh stay drawn, dimmer, until the next answer. */
function fadeMarineTraffic(m: maplibregl.Map, stale: boolean) {
  for (const id of ["live-mt-ship-dots", "live-mt-ships"]) if (m.getLayer(id)) m.setPaintProperty(id, id.endsWith("dots") ? "circle-opacity" : "icon-opacity", reportAge(stale ? 0.5 : 0.96));
}
/** Aircraft from a failed refresh stay drawn, faded, so they do not read as current. */
function fadeAircraft(m: maplibregl.Map, stale: boolean) {
  if (m.getLayer("live-aircraft")) m.setPaintProperty("live-aircraft", "icon-opacity", ["case", ["get", "on_ground"], stale ? 0.3 : 0.62, stale ? 0.45 : 1]);
  if (m.getLayer("live-aircraft-labels")) m.setPaintProperty("live-aircraft-labels", "text-opacity", stale ? 0.5 : 1);
  for (const id of ["live-aircraft-trail-civil", "live-aircraft-trail-military"]) if (m.getLayer(id)) m.setPaintProperty(id, "line-opacity", stale ? 0.4 : 1);
}
function viewParams(m: maplibregl.Map) {
  const b = m.getBounds();
  const wrap = (x: number) => ((((x + 180) % 360) + 360) % 360) - 180;
  let west = b.getWest(), east = b.getEast();
  if (east - west >= 359) { west = -180; east = 180; } else { west = wrap(west); east = wrap(east); }
  return new URLSearchParams({ west: west.toFixed(3), south: Math.max(-85, b.getSouth()).toFixed(3), east: east.toFixed(3), north: Math.min(85, b.getNorth()).toFixed(3), zoom: m.getZoom().toFixed(1) });
}
const flightKey = (p: Props) => String(p.flight_id ?? "");
const sameShip = (a: Props, b: Props) => !!((a.key && a.key === b.key) || (a.mmsi != null && String(a.mmsi) === String(b.mmsi)) || (a.id != null && a.id === b.id));

export default function LiveLayers({ map, ready, historical, historicalAt = null, onSelect }: { map: MutableRefObject<maplibregl.Map | null>; ready: boolean; historical: boolean; historicalAt?: string | null; onSelect: (id: string, revision: string) => void }) {
  const client = useQueryClient();
  const [settings, setSettingsState] = useState<Settings>(loadSettings);
  const setSettings = useCallback((update: (s: Settings) => Settings) => setSettingsState(update), []);
  const [open, setOpen] = useState(false);
  const [status, setStatus] = useState<Record<"ships" | "aircraft" | "warnings", Status>>({ ships: idle, aircraft: idle, warnings: idle });
  const [counts, setCounts] = useState<{ ships: Partial<Record<ShipCategory, number>>; movement: { moving: number; still: number } }>({ ships: {}, movement: { moving: 0, still: 0 } });
  const [selected, setSelected] = useState<Selected | null>(null);
  const [hover, setHover] = useState<Hover | null>(null);
  const [track, setTrack] = useState<Track>({ note: null, hours: null, series: null });
  const [following, setFollowing] = useState(false);
  // Warnings in the window, newest first, for the Layers panel list.
  const [warningList, setWarningList] = useState<Feature[]>([]);
  // A chosen past moment; a historical snapshot supplies one by default.
  const [chosenAt, setChosenAt] = useState<string | null>(null);
  const snapshotAt = historical && historicalAt ? snapMoment(historicalAt) : null;
  const past = chosenAt ?? snapshotAt;
  // When the moment falls in a stretch KeelWatch was not collecting.
  const [pastGap, setPastGap] = useState<{ from: string; to: string } | null>(null);
  // shipsFetched: when tiles of the current request first arrived (never when
  // they were only asked for); tiles: arrivals and failures since that request.
  const data = useRef({ aircraft: [] as Feature[], shipsFetched: null as string | null, shipVersion: Date.now(), shipBasis: "", tiles: { loaded: 0, failed: 0, error: null as string | null },
    aircraftRequests: 1, aircraftAt: 0, aircraftFailures: 0, aircraftFaded: false,
    // MarineTraffic: the drawn view, its answer's state, and the refresh backoff.
    mt: [] as Feature[], mtAt: 0, mtFailures: 0, mtFaded: false, mtState: { acquired: null as string | null, error: null as string | null, stale: false, incomplete: false, note: null as string | null } });
  const latest = useRef({ settings, historical, selected, following, past });
  latest.current = { settings, historical, selected, following, past };
  const abort = useRef<Record<string, AbortController | undefined>>({});
  /** Each drawn aircraft's recent reports: its trail and climb rate. */
  const history = useRef(new AircraftHistory());
  const toggle = useRef<HTMLButtonElement>(null);
  /** Keyboard picks: each one gives its card heading focus once. */
  const picks = useRef(0);
  useEffect(() => { try { localStorage.setItem(STORE, JSON.stringify(settings)); } catch { /* per-viewer convenience only */ } }, [settings]);

  const ring = useCallback((at: [number, number] | null) => {
    (map.current?.getSource("live-selected") as maplibregl.GeoJSONSource | undefined)?.setData(at ? { type: "FeatureCollection", features: [{ type: "Feature", properties: {}, geometry: { type: "Point", coordinates: at } }] } : empty);
  }, [map]);
  /** Keep a followed target centred. Only a real move eases the camera: easing
   *  to where the map already is would fire camera events and feed back here. */
  const followTo = useCallback((at: [number, number]) => {
    const m = map.current; if (!m || !latest.current.following) return;
    const c = m.project(m.getCenter()), p = m.project(at);
    if (Math.hypot(p.x - c.x, p.y - c.y) < FOLLOW_SLACK_PX) return;
    m.easeTo({ center: at, duration: 800 }, { liveFollow: true } satisfies Own);
  }, [map]);

  /** Ships in view from the loaded tiles: counts, and the selected ship's latest report. */
  const readShips = useRef<() => void>(() => {});
  readShips.current = () => {
    const m = map.current; const { settings: st, past, selected: sel } = latest.current;
    if (!m?.getSource("live-ships") || !st.ships) return;
    if (st.shipSource === "marinetraffic") { readMarineTraffic.current(); return; }
    const bounds = m.getBounds();
    const seen = new Set<string>();
    const cats: Partial<Record<ShipCategory, number>> = {};
    let total = 0, merged = false, move = 0, still = 0;
    const passes = (p: Props) => (st.shipCategories as string[]).includes(String(p.cat ?? "other")) && (st.movement === "all" || (st.movement === "moving") === (Number(p.sog ?? 0) >= 0.5));
    for (const f of m.querySourceFeatures("live-ships", { sourceLayer: "vessels" })) {
      const key = String(f.id ?? f.properties?.id ?? "");
      if (!key || seen.has(key)) continue; seen.add(key);
      const at = (f.geometry as Point).coordinates as [number, number];
      if (!inView(bounds, at)) continue;
      const p = f.properties as Props; const n = Number(p.n ?? 1);
      if (n > 1) merged = true;
      const cat = String(p.cat ?? "other") as ShipCategory;
      cats[cat] = (cats[cat] ?? 0) + n;
      if (!passes(p)) continue;
      total += n; if (Number(p.sog ?? 0) >= 0.5) move += n; else still += n;
    }
    setCounts({ ships: cats, movement: { moving: move, still } });
    // Failed tiles are named: some failed (incomplete), or none of this request arrived (stale).
    const t = data.current.tiles, acquired = past ?? data.current.shipsFetched;
    setStatus(s => ({ ...s, ships: { ...idle, count: total, total: merged ? total : null, mode: merged ? "density" : "vessels", acquired, loading: !acquired && !t.error, error: t.error, stale: !!t.error && !t.loaded && !!acquired, incomplete: t.failed > 0 && t.loaded > 0 } }));
    if (past) return;
    // Keep the selection ring and card on the selected ship's latest report.
    if (sel?.kind === "ship") {
      const where: maplibregl.FilterSpecification = sel.props.mmsi != null ? ["==", ["id"], Number(sel.props.mmsi)] : ["==", ["get", "id"], String(sel.props.id ?? "")];
      const hit = m.querySourceFeatures("live-ships", { sourceLayer: "vessels", filter: where })[0];
      if (hit) {
        const at = (hit.geometry as Point).coordinates as [number, number];
        ring(at); followTo(at);
        const next = shipProps(hit.id, hit.properties as Props, at);
        setSelected(s => s && s.kind === "ship" && sameShip(s.props, next) ? { ...s, props: { ...s.props, ...Object.fromEntries(Object.entries(next).filter(([, v]) => v != null)), key: s.props.key }, inView: true } : s);
      }
    }
  };

  /** MarineTraffic's ships in view: counts by type and movement, the status
   *  line, and the selected ship's latest report. */
  const readMarineTraffic = useRef<() => void>(() => {});
  readMarineTraffic.current = () => {
    const m = map.current; const { settings: st, selected: sel } = latest.current;
    if (!m) return;
    const bounds = m.getBounds();
    const cats: Partial<Record<ShipCategory, number>> = {};
    let total = 0, move = 0, still = 0;
    for (const f of data.current.mt) {
      const at = (f.geometry as Point).coordinates as [number, number];
      if (!inView(bounds, at)) continue;
      const p = (f.properties ?? {}) as Props;
      const cat = String(p.cat ?? "other") as ShipCategory;
      cats[cat] = (cats[cat] ?? 0) + 1;
      if (!(st.shipCategories as string[]).includes(cat)) continue;
      const going = Number(p.sog ?? 0) >= 0.5;
      if (st.movement !== "all" && (st.movement === "moving") !== going) continue;
      total += 1; if (going) move += 1; else still += 1;
    }
    setCounts({ ships: cats, movement: { moving: move, still } });
    const x = data.current.mtState;
    setStatus(s => ({ ...s, ships: { ...idle, count: total, mode: "vessels", acquired: x.acquired, loading: !x.acquired && !x.error && !x.note, error: x.error, stale: x.stale, incomplete: x.incomplete, note: x.note ?? undefined } }));
    if (sel?.kind !== "ship" || sel.props.provider !== "marinetraffic") return;
    const hit = data.current.mt.find(f => (f.properties as Props | null)?.id === sel.props.key);
    if (hit) {
      const at = (hit.geometry as Point).coordinates as [number, number];
      ring(at); followTo(at);
      const next = shipProps(undefined, hit.properties as Props, at);
      setSelected(s => s && s.kind === "ship" && sameShip(s.props, next) ? { ...s, props: { ...s.props, ...Object.fromEntries(Object.entries(next).filter(([, v]) => v != null)), key: s.props.key }, inView: true } : s);
    } else setSelected(s => s && s.inView ? { ...s, inView: false } : s);
  };
  /** Ask Relic for MarineTraffic's ships in the current view. Nothing is
   *  asked at a past moment or while another source is chosen. */
  const refreshMarineTraffic = useRef<() => void>(() => {});
  refreshMarineTraffic.current = () => {
    const m = map.current; const source = m?.getSource("live-mt-ships") as maplibregl.GeoJSONSource | undefined;
    if (!m || !source) return;
    const { settings: st, past } = latest.current;
    abort.current.mt?.abort();
    const d = data.current;
    const clearWith = (note: string | null) => { source.setData(empty); d.mt = []; d.mtState = { acquired: null, error: null, stale: false, incomplete: false, note }; };
    if (!st.ships || st.shipSource !== "marinetraffic") { clearWith(null); return; }
    if (past) { clearWith("Live only: Relic keeps no MarineTraffic history"); readMarineTraffic.current(); return; }
    const controller = new AbortController(); abort.current.mt = controller;
    setStatus(s => ({ ...s, ships: { ...s.ships, loading: true } }));
    const fade = (stale: boolean) => { if (d.mtFaded !== stale) { d.mtFaded = stale; fadeMarineTraffic(m, stale); } };
    api(`/layers/ships/marinetraffic?${viewParams(m)}`, mtSchema, { signal: controller.signal }).then(body => {
      if (controller.signal.aborted) return;
      d.mtAt = Date.now();
      const zoomIn = typeof body.meta?.zoom_in === "string" ? body.meta.zoom_in : null;
      if (zoomIn) { clearWith(zoomIn === "view_width" ? "Zoom in to show MarineTraffic ships: this view needs too many of its map tiles" : "Zoom in to show MarineTraffic ships"); d.mtFailures = 0; fade(false); readMarineTraffic.current(); return; }
      const features = body.features as Feature[];
      source.setData({ type: "FeatureCollection", features }); d.mt = features;
      d.mtFailures = body.stale ? d.mtFailures + 1 : 0; fade(!!body.stale);
      d.shipsFetched = body.acquired_at ?? null;
      d.mtState = { acquired: body.acquired_at ?? null, error: body.error ?? null, stale: !!body.stale, incomplete: !!body.incomplete, note: null };
      readMarineTraffic.current();
    }).catch(error => {
      if (controller.signal.aborted) return;
      // What is still drawn is from an earlier answer: dimmer, said, retried later.
      d.mtAt = Date.now(); d.mtFailures += 1; fade(d.mt.length > 0);
      d.mtState = { ...d.mtState, error: error instanceof Error ? error.message : "MarineTraffic unavailable", stale: d.mt.length > 0, note: null };
      readMarineTraffic.current();
    });
  };

  const refresh = useRef<(layer: "ships" | "aircraft" | "warnings") => void>(() => {});
  refresh.current = (layer) => {
    const m = map.current; if (!m || !m.getSource("live-aircraft")) return;
    const { settings: st, past } = latest.current;
    if (layer === "ships") {
      shipVisibility(m, st, past);
      if (!st.ships) { data.current.shipBasis = ""; refreshMarineTraffic.current(); setStatus(s => ({ ...s, ships: idle })); setCounts({ ships: {}, movement: { moving: 0, still: 0 } }); return; }
      // MarineTraffic: the view's ships from Relic; KeelWatch's tiles are not asked for.
      if (st.shipSource === "marinetraffic") {
        if (data.current.shipBasis !== "marinetraffic") { data.current.shipBasis = "marinetraffic"; data.current.shipsFetched = null; data.current.tiles = { loaded: 0, failed: 0, error: null }; setStatus(s => ({ ...s, ships: { ...idle, loading: true } })); }
        refreshMarineTraffic.current();
        return;
      }
      refreshMarineTraffic.current();
      // A past moment's tiles do not change once it is six hours old; live
      // tiles get a new version so the browser fetches fresh ones.
      if (!past) data.current.shipVersion = Date.now();
      // Another moment or freshness: nothing shown yet belongs to it.
      const basis = `${past ?? "live"}|${st.freshness}`;
      if (basis !== data.current.shipBasis) { data.current.shipBasis = basis; data.current.shipsFetched = null; data.current.tiles.error = null; setStatus(s => ({ ...s, ships: { ...s.ships, acquired: null, error: null, loading: true } })); }
      data.current.tiles.loaded = 0; data.current.tiles.failed = 0;
      (m.getSource("live-ships") as maplibregl.VectorTileSource).setTiles([tileUrl(st.freshness, data.current.shipVersion, past)]);
      return;
    }
    if (layer === "warnings") {
      const source = m.getSource("live-warnings") as maplibregl.GeoJSONSource;
      abort.current.warnings?.abort();
      if (!st.warnings) { source.setData(empty); setWarningList([]); setStatus(s => ({ ...s, warnings: idle })); return; }
      const controller = new AbortController(); abort.current.warnings = controller;
      setStatus(s => ({ ...s, warnings: { ...s.warnings, loading: true } }));
      // The server counts the hours back from now, or from a past moment (then
      // only warnings registered by it, with the listing recorded as of it),
      // and says how many it retains, so a short window is never read as all.
      const q = new URLSearchParams();
      if (st.warningHours !== "all") q.set("hours", st.warningHours);
      if (past) q.set("at", past);
      api(`/layers/warnings?${q}`, aircraftViewSchema, { signal: controller.signal }).then(body => {
        if (controller.signal.aborted) return;
        const now = past ? Date.parse(past) : Date.now();
        const inRange = body.features as Feature[];
        const unlisted = inRange.filter((f: Feature) => f.properties?.listed === false).length;
        const features = inRange.filter((f: Feature) => st.unlistedWarnings || f.properties?.listed !== false).map((f: Feature) => ({ ...f, properties: { ...f.properties, age_days: (now - Date.parse(String(f.properties?.registered_at))) / 86_400_000 } }));
        source.setData({ type: "FeatureCollection", features }); setWarningList(features);
        const w = (body as { status?: Record<string, unknown> }).status ?? {};
        const retained = typeof body.retained === "number" ? body.retained : undefined;
        setStatus(s => ({ ...s, warnings: { ...idle, count: features.length, retained, unlisted: st.unlistedWarnings ? unlisted : 0, hiddenUnlisted: st.unlistedWarnings ? 0 : unlisted, acquired: typeof w.last_success_at === "string" ? w.last_success_at : null, stale: w.ok === false, error: typeof w.error === "string" ? w.error : null } }));
      }).catch(error => { if (!controller.signal.aborted) setStatus(s => ({ ...s, warnings: { ...s.warnings, loading: false, error: error instanceof Error ? error.message : "Warnings unavailable" } })); });
      return;
    }
    const source = m.getSource("live-aircraft") as maplibregl.GeoJSONSource;
    abort.current.aircraft?.abort();
    const services = AIRCRAFT_GROUPS.filter(g => st.aircraftGroups.includes(g.key)).flatMap(g => g.services);
    const trails = m.getSource("live-aircraft-trails") as maplibregl.GeoJSONSource | undefined;
    if (!st.aircraft || past || !services.length || m.getZoom() < 2) { trails?.setData(empty); history.current.clear(); }
    if (!st.aircraft || past || !services.length) { source.setData(empty); data.current.aircraft = []; setStatus(s => ({ ...s, aircraft: st.aircraft && past ? { ...idle, note: "Live only: Relic keeps no aircraft history" } : st.aircraft && !services.length ? { ...idle, note: "No aircraft types selected" } : idle })); return; }
    if (m.getZoom() < 2) { source.setData(empty); data.current.aircraft = []; setStatus(s => ({ ...s, aircraft: { ...idle, note: "Zoom in to show aircraft" } })); return; }
    const q = viewParams(m); q.set("services", services.join(","));
    const controller = new AbortController(); abort.current.aircraft = controller;
    setStatus(s => ({ ...s, aircraft: { ...s.aircraft, loading: true } }));
    const fade = (stale: boolean) => { if (data.current.aircraftFaded !== stale) { data.current.aircraftFaded = stale; fadeAircraft(m, stale); } };
    api(`/layers/aircraft?${q}`, aircraftViewSchema, { signal: controller.signal }).then(body => {
      if (controller.signal.aborted) return;
      const features = aircraftFeatures(body.features as Feature[]);
      // Sets each feature's climb rate before it is drawn or hovered.
      trails?.setData({ type: "FeatureCollection", features: history.current.update(features) });
      source.setData({ type: "FeatureCollection", features }); data.current.aircraft = features;
      const meta = body.meta ?? {};
      // A stale answer is the server's last view after a failed request: it backs off too.
      data.current.aircraftRequests = Number(meta.requests ?? 1) || 1; data.current.aircraftAt = Date.now();
      data.current.aircraftFailures = body.stale ? data.current.aircraftFailures + 1 : 0; fade(!!body.stale);
      setStatus(s => ({ ...s, aircraft: { ...idle, count: features.length, military: features.filter(f => f.properties?.tone === "military").length, acquired: body.acquired_at ?? null, incomplete: !!meta.possibly_truncated, stale: !!body.stale, error: body.error ?? null } }));
      const sel = latest.current.selected;
      if (sel?.kind === "aircraft") {
        const hit = features.find(f => flightKey((f.properties ?? {}) as Props) === flightKey(sel.props));
        if (hit) {
          const at = (hit.geometry as Point).coordinates as [number, number];
          ring(at); followTo(at);
          setSelected(s => s && s.kind === "aircraft" && flightKey(s.props) === flightKey(sel.props) ? { ...s, props: hit.properties as Props, inView: true } : s);
        } else setSelected(s => s && s.inView ? { ...s, inView: false } : s);
      }
    }).catch(error => {
      if (controller.signal.aborted) return;
      // What is still drawn is from an earlier refresh: faded, labelled stale, retried later.
      data.current.aircraftAt = Date.now(); data.current.aircraftFailures += 1; fade(data.current.aircraft.length > 0);
      setStatus(s => ({ ...s, aircraft: { ...s.aircraft, loading: false, stale: data.current.aircraft.length > 0, error: error instanceof Error ? error.message : "Aircraft unavailable" } }));
    });
  };

  // Type and movement filters apply on the map without refetching.
  useEffect(() => {
    const m = map.current; if (!ready || !m?.getLayer("live-ship-dots")) return;
    const f = shipFilter(settings);
    for (const id of [...SHIP_LAYERS, ...MT_LAYERS]) if (m.getLayer(id)) m.setFilter(id, f);
    if (m.getLayer("live-aircraft")) m.setFilter("live-aircraft", settings.onGround ? null : ["!", ["get", "on_ground"]]);
    if (m.getLayer("live-aircraft-alert")) m.setFilter("live-aircraft-alert", settings.onGround ? ["==", ["get", "emergency"], true] : ["all", ["==", ["get", "emergency"], true], ["!", ["get", "on_ground"]]]);
    readShips.current();
  }, [ready, settings.movement, settings.onGround, settings.shipCategories, map]);

  // The latest track request; an older answer (or one for another selection) is dropped.
  const trackRun = useRef<{ id: number; controller?: AbortController }>({ id: 0 });
  const stopTrack = useCallback(() => { trackRun.current.controller?.abort(); trackRun.current = { id: trackRun.current.id + 1 }; }, []);
  /** Select a feature. Choosing the selected one again keeps its track and follow. */
  const select = useCallback((kind: Selected["kind"], props: Props, point?: [number, number], focus = 0) => {
    const m = map.current; if (!m) return;
    const prev = latest.current.selected;
    const same = !!prev && prev.kind === kind && (kind === "ship" ? sameShip(prev.props, props) : kind === "aircraft" ? flightKey(prev.props) === flightKey(props) : prev.props.id === props.id);
    setSelected({ kind, props, inView: true, focus });
    ring(point && kind !== "warning" ? point : null);
    if (same) return;
    setFollowing(false); setTrack({ note: null, hours: null, series: null }); stopTrack();
    (m.getSource("live-track") as maplibregl.GeoJSONSource | undefined)?.setData(empty);
  }, [map, ring, stopTrack]);
  const clear = useCallback(() => {
    setSelected(null); setFollowing(false); setTrack({ note: null, hours: null, series: null }); stopTrack();
    for (const id of ["live-track", "live-selected"]) (map.current?.getSource(id) as maplibregl.GeoJSONSource | undefined)?.setData(empty);
  }, [map, stopTrack]);
  const clearRef = useRef(clear); clearRef.current = clear;

  useEffect(() => {
    const m = map.current; if (!ready || !m) return;
    const setup = () => { install(m, latest.current.settings, data.current.shipVersion, latest.current.past); data.current.aircraftFaded = false; data.current.mtFaded = false; refresh.current("ships"); refresh.current("aircraft"); refresh.current("warnings"); };
    setup();
    let timer = 0, frame = 0, counting = 0;
    // A follow move is this code's own: it must not refresh aircraft (which
    // would follow again) or bypass the aircraft refresh limit.
    const moved = (e: unknown) => { if (own(e)) return; window.clearTimeout(timer); timer = window.setTimeout(() => { refresh.current("aircraft"); if (latest.current.settings.shipSource === "marinetraffic") refreshMarineTraffic.current(); }, 350); };
    const settled = () => { window.clearTimeout(counting); counting = window.setTimeout(() => readShips.current(), 250); };
    // Ship tiles: arrival dates the view; a failure is said, never shown as an empty sea.
    const tileLoaded = (e: maplibregl.MapSourceDataEvent) => {
      if (e.sourceId !== "live-ships" || !e.tile) return;
      const t = data.current.tiles; t.loaded += 1;
      if (t.loaded === 1) data.current.shipsFetched = latest.current.past ?? new Date().toISOString();
      if (!t.failed) t.error = null;
    };
    const tileFailed = (e: maplibregl.ErrorEvent & { sourceId?: string }) => {
      if (e.sourceId !== "live-ships") return;
      const t = data.current.tiles; t.failed += 1;
      if (t.failed > 1) return;
      const basis = data.current.shipBasis;
      t.error = "Ship tiles failed. Ships here were not received, not absent.";
      settled();
      void tileFailure(e.error).then(message => { if (data.current.shipBasis === basis && t.error) { t.error = message; settled(); } });
    };
    const kindOf = (layer: string): Hover["kind"] => layer === "live-aircraft" ? "aircraft" : layer === "live-warnings" ? "warning" : "ship";
    const propsOf = (f: maplibregl.MapGeoJSONFeature): Props => {
      const at = (f.geometry as Point).coordinates as [number, number];
      if (kindOf(f.layer.id) === "ship") return shipProps(f.id, f.properties as Props, at);
      if (kindOf(f.layer.id) === "aircraft") return (data.current.aircraft.find(x => flightKey((x.properties ?? {}) as Props) === flightKey(f.properties as Props))?.properties as Props) ?? (f.properties as Props);
      return f.properties as Props;
    };
    // One pending frame reads the latest pointer; a busy map never starves it.
    let pointer: maplibregl.MapMouseEvent | null = null;
    const move = (event: maplibregl.MapMouseEvent) => {
      pointer = event;
      if (frame) return;
      frame = requestAnimationFrame(() => {
        frame = 0; const e = pointer; if (!e) return;
        const f = liveFeatureNear(m, e.point);
        if (f) m.getCanvas().style.cursor = "pointer";
        const rect = m.getContainer().getBoundingClientRect();
        setHover(f ? { kind: kindOf(f.layer.id), props: propsOf(f), x: rect.left + e.point.x, y: rect.top + e.point.y } : null);
      });
    };
    const leave = (e?: unknown) => { if (own(e)) return; cancelAnimationFrame(frame); frame = 0; pointer = null; setHover(null); };
    const click = (e: maplibregl.MapMouseEvent) => {
      const f = liveFeatureNear(m, e.point);
      // A click anywhere else on the map ends the selection, like closing the card.
      if (!f) { if (latest.current.selected) clearRef.current(); return; }
      const at = (f.geometry as Point).coordinates as [number, number];
      select(kindOf(f.layer.id), propsOf(f), at);
      setOpen(false);
    };
    const out = () => leave();
    m.on("style.load", setup); m.on("moveend", moved); m.on("idle", settled); m.on("mousemove", move); m.on("click", click); m.on("movestart", leave);
    m.on("sourcedata", tileLoaded); m.on("error", tileFailed);
    m.getCanvas().addEventListener("mouseleave", out);
    const warnings = window.setInterval(() => { if (!document.hidden) refresh.current("warnings"); }, 300_000);
    // An emergency ring breathes, slowly, only while one is drawn.
    let bright = true;
    const pulse = window.setInterval(() => {
      if (document.hidden || !m.getLayer("live-aircraft-alert") || !data.current.aircraft.some(f => f.properties?.emergency === true)) return;
      bright = !bright;
      m.setPaintProperty("live-aircraft-alert", "circle-stroke-opacity", bright ? 0.95 : 0.25);
      m.setPaintProperty("live-aircraft-alert", "circle-opacity", bright ? 0.16 : 0.04);
    }, 900);
    const ships = window.setInterval(() => { if (!document.hidden && !latest.current.past && latest.current.settings.shipSource !== "marinetraffic") refresh.current("ships"); }, SHIP_REFRESH_MS);
    const marine = window.setInterval(() => {
      const d = data.current, { settings: st, past } = latest.current;
      if (document.hidden || past || !st.ships || st.shipSource !== "marinetraffic") return;
      const due = d.mtFailures ? MT_BACKOFF_MS[Math.min(d.mtFailures, MT_BACKOFF_MS.length) - 1] : MT_REFRESH_MS;
      if (Date.now() - d.mtAt >= due - 500) refreshMarineTraffic.current();
    }, 5_000);
    // A wide view costs many provider requests; refresh it less often so the
    // proxied Flightradar24 feed sees a modest, steady rate (about one request
    // every 3 s at most). After failures the next tries wait 30 s, 60 s, then 120 s.
    const planes = window.setInterval(() => {
      const d = data.current;
      const due = d.aircraftFailures ? AIRCRAFT_BACKOFF_MS[Math.min(d.aircraftFailures, AIRCRAFT_BACKOFF_MS.length) - 1] : Math.max(10_000, d.aircraftRequests * 3_000);
      if (!document.hidden && Date.now() - d.aircraftAt >= due - 500) refresh.current("aircraft");
    }, 5_000);
    return () => {
      window.clearTimeout(timer); window.clearTimeout(counting); cancelAnimationFrame(frame);
      window.clearInterval(ships); window.clearInterval(planes); window.clearInterval(warnings); window.clearInterval(pulse); window.clearInterval(marine);
      m.off("style.load", setup); m.off("moveend", moved); m.off("idle", settled); m.off("mousemove", move); m.off("click", click); m.off("movestart", leave);
      m.off("sourcedata", tileLoaded); m.off("error", tileFailed);
      m.getCanvas().removeEventListener("mouseleave", out);
    };
  }, [ready, map, select]);
  useEffect(() => { refresh.current("ships"); }, [settings.ships, settings.freshness, settings.shipSource, past]);
  useEffect(() => { refresh.current("aircraft"); }, [settings.aircraft, settings.aircraftGroups, past]);
  useEffect(() => { refresh.current("warnings"); }, [settings.warnings, settings.warningHours, settings.unlistedWarnings, past]);
  // Provider health for the Layers panel: checked while it is open.
  const health = useQuery({ queryKey: ["layer-health"], enabled: open, refetchInterval: open ? 60_000 : false, staleTime: 20_000, retry: false,
    queryFn: ({ signal }) => api("/layers", layersSchema, { signal }) });
  const healthById = Object.fromEntries((health.data?.layers ?? []).flatMap(l => l.health ? [[l.id, l.health as LayerHealth]] : []));
  // The map status opens this panel when only layers need attention.
  useEffect(() => {
    const show = () => { setOpen(true); void client.invalidateQueries({ queryKey: ["layer-health"] }); };
    window.addEventListener("relic:open-layers", show);
    return () => window.removeEventListener("relic:open-layers", show);
  }, [client]);
  // Moving between live and a past moment changes what a selection means.
  useEffect(() => { clear(); setHover(null); }, [past, clear]);
  // A ship from the other source is no longer drawn once the source changes.
  useEffect(() => { const sel = latest.current.selected; if (sel?.kind === "ship" && (sel.props.provider === "marinetraffic") !== (settings.shipSource === "marinetraffic")) { clear(); setHover(null); } }, [settings.shipSource, clear]);
  // Say so when KeelWatch was not collecting at the moment: an emptier sea
  // there is a gap in collection, not an absence of ships.
  useEffect(() => {
    setPastGap(null);
    if (!past || settings.shipSource !== "keelwatch") return;
    const controller = new AbortController();
    // Two days either side, so the whole gap around the moment is named.
    const from = new Date(Date.parse(past) - 48 * 3_600_000).toISOString();
    const to = new Date(Math.min(Date.now(), Date.parse(past) + 48 * 3_600_000)).toISOString();
    api(`/layers/ships/coverage?from=${encodeURIComponent(from)}&to=${encodeURIComponent(to)}`, coverageSchema, { signal: controller.signal })
      .then(c => setPastGap(c.gaps.find(g => Date.parse(g.from) <= Date.parse(past) && Date.parse(g.to) >= Date.parse(past)) ?? null))
      .catch(() => { /* coverage is advisory; the map still shows what was stored */ });
    return () => controller.abort();
  }, [past, settings.shipSource]);
  const choose = useCallback((at: string | null) => setChosenAt(at ? snapMoment(at) : null), []);

  // Tracks: a ship's KeelWatch history, an aircraft's provider trail. A range
  // (from the ship finder) shows the track around a message's time instead.
  // Each request replaces the one before: a slow older answer never draws.
  const drawTrack = useCallback(async (sel: Selected, hours: number, given?: { from: string; to: string }) => {
    const m = map.current; if (!m) return;
    trackRun.current.controller?.abort();
    const controller = new AbortController(), run = trackRun.current.id + 1;
    trackRun.current = { id: run, controller };
    // At a past moment the track spans the hours either side of it.
    const at = latest.current.past;
    const range = given ?? (at && sel.kind === "ship" ? { from: new Date(Date.parse(at) - hours * 3_600_000).toISOString(), to: new Date(Math.min(Date.now(), Date.parse(at) + hours * 3_600_000)).toISOString() } : undefined);
    const source = m.getSource("live-track") as maplibregl.GeoJSONSource;
    const still = () => { const now = latest.current.selected; return trackRun.current.id === run && !!now && now.kind === sel.kind && (sel.kind === "ship" ? sameShip(now.props, sel.props) : flightKey(now.props) === flightKey(sel.props)); };
    // Fit the map to the track around a message, or when the selection has no position of its own.
    const fit = !!given || !position(sel.props.lon, sel.props.lat);
    if (sel.kind === "ship" && sel.props.provider === "marinetraffic") {
      source.setData(empty);
      setTrack({ note: null, hours: null, series: null });
    } else if (sel.kind === "ship") {
      const key = String(sel.props.key ?? "");
      const span = range ? `from ${range.from.slice(0, 16).replace("T", " ")} to ${range.to.slice(0, 16).replace("T", " ")} UTC${given ? ", around the message" : ""}` : null;
      setTrack({ note: span ? `Loading the track ${span}…` : `Loading ${hours < 48 ? `${hours} h` : `${hours / 24} days`} of KeelWatch history…`, hours: given ? null : hours, series: null });
      try {
        const query = range ? `from=${encodeURIComponent(range.from)}&to=${encodeURIComponent(range.to)}` : `hours=${hours}`;
        const body = await api(`/layers/ships/${encodeURIComponent(key)}/track?${query}`, z.object({ items: z.array(z.record(z.string(), z.any())).nullish(), meta: z.record(z.string(), z.any()).nullish() }).passthrough(), { signal: controller.signal });
        if (!still()) return;
        // Only fixes with a real position and time; a missing coordinate is never 0.
        const points: Sample[] = (body.items ?? []).flatMap(i => { const p = position(i.longitude, i.latitude), t = Date.parse(String(i.observed_at)) / 1000; return p && Number.isFinite(t) ? [{ t, lon: p[0], lat: p[1], v: finite(i.speed_knots) }] : []; });
        source.setData({ type: "FeatureCollection", features: coloured(valueSegments(points, 3 * 3600, 60), SPEED_RAMP) });
        const count = `${points.length.toLocaleString("en-GB")} reported positions${span ? ` ${span}` : ""}${body.meta?.truncated ? " (history truncated)" : ""}.`;
        setTrack({ hours: given ? null : hours, series: points.map(p => ({ t: p.t, v: p.v })), note: points.length ? `${count} Coloured by reported speed; gaps over 3 h stay open.` : "No positions collected in this period. That does not mean the vessel did not move." });
        if (fit && points.length) {
          const b = new maplibregl.LngLatBounds();
          for (const p of points) b.extend([p.lon, p.lat]);
          m.fitBounds(b, { padding: 120, maxZoom: 10, duration: 900 });
        }
      } catch (e) { if (still()) setTrack({ hours: null, series: null, note: e instanceof Error ? e.message : "History unavailable" }); }
    } else if (sel.kind === "aircraft") {
      try {
        const flight = await client.fetchQuery({ queryKey: ["live-flight", Number(sel.props.flight_id)], staleTime: 20_000, queryFn: ({ signal }) => api(`/layers/aircraft/${Number(sel.props.flight_id)}`, z.record(z.string(), z.any()), { signal }) });
        if (!still()) return;
        const trail: Sample[] = ((flight.trail ?? []) as { t: unknown; lon: unknown; lat: unknown; altitude_ft?: unknown }[]).flatMap(p => { const at = position(p.lon, p.lat), t = finite(p.t); return at && t != null ? [{ t, lon: at[0], lat: at[1], v: finite(p.altitude_ft) }] : []; });
        source.setData({ type: "FeatureCollection", features: coloured(valueSegments(trail, 15 * 60, 700, false), ALTITUDE_RAMP) });
        setTrack({ hours: null, series: trail.map(p => ({ t: p.t, v: p.v })), note: trail.length ? `Path from ${trail.length.toLocaleString("en-GB")} provider positions, coloured by altitude; gaps over 15 min stay open.` : "The provider sent no flight path." });
      } catch (e) { if (still()) setTrack({ hours: null, series: null, note: e instanceof Error ? e.message : "Flight path unavailable" }); }
    }
  }, [map, client]);
  const selectedKey = selected ? `${selected.kind}:${selected.kind === "ship" ? selected.props.key : selected.kind === "aircraft" ? flightKey(selected.props) : selected.props.id}` : "";
  // The ship finder draws its own track for the vessel it shows; the next
  // selection change, whatever it is, clears that mark.
  const drawnFor = useRef<string | null>(null);
  useEffect(() => {
    const drawn = drawnFor.current === selectedKey; drawnFor.current = null;
    if (!selected || selected.kind === "warning" || drawn) return;
    void drawTrack(selected, selected.kind === "ship" ? (latest.current.past ? 6 : 24) : 0);
  }, [selectedKey]); // eslint-disable-line react-hooks/exhaustive-deps

  // The ship finder asks to show a vessel with its track around a message.
  useEffect(() => {
    const show = (event: Event) => {
      const d = (event as CustomEvent).detail as Props & { from?: unknown; to?: unknown };
      const m = map.current; if (!m || !d?.key) return;
      const at = position(d.lon, d.lat);
      setSettings(s => ({ ...s, ships: true, shipSource: "keelwatch" }));
      const sel: Selected = { kind: "ship", props: { key: d.key, id: d.key, name: d.name, category: d.category ?? "other", mmsi: d.mmsi ?? null, imo: d.imo ?? null, ...(at ? { lon: at[0], lat: at[1] } : {}) }, inView: true };
      select("ship", sel.props, at ?? undefined);
      // Drawn here, not by the selection effect: showing the vessel already
      // selected changes no key, and its track must still be the message's.
      drawnFor.current = `ship:${String(d.key)}`;
      if (typeof d.from === "string" && typeof d.to === "string") void drawTrack(sel, 0, { from: d.from, to: d.to });
      else void drawTrack(sel, latest.current.past ? 6 : 24);
      if (at) m.flyTo({ center: at, zoom: Math.max(m.getZoom(), 7), duration: 900, essential: false });
    };
    // Every ship KeelWatch recorded at a moment (from the reader): a past view there.
    const at = (event: Event) => {
      const d = (event as CustomEvent).detail as { at: string; lon?: unknown; lat?: unknown };
      const m = map.current; if (!m || !d?.at) return;
      setSettings(s => ({ ...s, ships: true, shipSource: "keelwatch" }));
      setChosenAt(snapMoment(d.at));
      const place = position(d.lon, d.lat);
      if (place) m.flyTo({ center: place, zoom: Math.max(m.getZoom(), 8), duration: 900, essential: false });
      setOpen(true);
    };
    window.addEventListener("relic:show-vessel", show);
    window.addEventListener("relic:ships-at", at);
    return () => { window.removeEventListener("relic:show-vessel", show); window.removeEventListener("relic:ships-at", at); };
  }, [map, select, setSettings, drawTrack]);

  // Escape closes the panel, then the selection, unless it was meant for
  // another dialog, menu or field, or the search box used it to clear itself.
  useEffect(() => {
    const key = (e: KeyboardEvent) => {
      if (e.key !== "Escape" || e.defaultPrevented) return;
      const t = e.target instanceof Element ? e.target : null;
      const ours = !!t?.closest(".live-layers, .live-card");
      if (!ours && (t?.closest("dialog, [role=dialog], [role=alertdialog], [role=menu], [role=listbox], input, textarea, select, [contenteditable=true]") || document.querySelector("dialog[open], [aria-modal=true]"))) return;
      if (open) { setOpen(false); if (t?.closest(".live-panel")) toggle.current?.focus(); }
      else if (latest.current.selected) clear();
    };
    window.addEventListener("keydown", key); return () => window.removeEventListener("keydown", key);
  }, [open, clear]);

  const local = useCallback((query: string) => {
    const m = map.current; const q = query.toLowerCase(); const digits = /^\d+$/.test(q);
    const seen = new Set<string>();
    const st = latest.current.settings;
    const drawn = !m || !st.ships ? [] : st.shipSource === "marinetraffic" ? data.current.mt.map(f => shipProps(undefined, f.properties as Props, (f.geometry as Point).coordinates as [number, number]))
      : m.getSource("live-ships") ? m.querySourceFeatures("live-ships", { sourceLayer: "vessels" }).map(f => shipProps(f.id, f.properties as Props, (f.geometry as Point).coordinates as [number, number])) : [];
    const ships = drawn
      .filter(p => { const k = String(p.key); if (!p.name || seen.has(k)) return false; seen.add(k); return digits ? String(p.mmsi ?? "").startsWith(q) || String(p.imo ?? "").startsWith(q) : String(p.name).toLowerCase().includes(q) || String(p.callsign ?? "").toLowerCase().startsWith(q); }).slice(0, 10);
    const aircraft = data.current.aircraft.map(f => f.properties as Props).filter(p => ["callsign", "flight_number", "registration"].some(k => String(p[k] ?? "").toLowerCase().startsWith(q))).slice(0, 10);
    return { ships, aircraft };
  }, [map]);
  const pick = useCallback(({ kind, props, keyboard }: Pick) => {
    const m = map.current; if (!m) return;
    // At a past moment a KeelWatch search result is today's report: its
    // position and motion are dropped; the track around the moment shows where it was.
    const then = latest.current.past && kind === "ship" && props.remote ? Object.fromEntries(Object.entries(props).filter(([k]) => !CURRENT_ONLY.has(k))) : props;
    const at = position(then.lon, then.lat);
    const category = (then.category as ShipCategory) ?? "other";
    // A KeelWatch result is drawn by KeelWatch; a MarineTraffic ship keeps its source.
    setSettings(s => kind === "ship" ? { ...s, ships: true, shipSource: then.provider === "marinetraffic" ? "marinetraffic" : "keelwatch", movement: "all", shipCategories: s.shipCategories.includes(category) ? s.shipCategories : [...s.shipCategories, category] } : { ...s, aircraft: true });
    const p = kind === "ship" ? { ...then, key: then.key ?? then.id } : then;
    select(kind, p, at ?? undefined, keyboard ? ++picks.current : 0);
    if (at) m.flyTo({ center: at, zoom: Math.max(m.getZoom(), kind === "ship" ? 10 : 8), duration: 1200, essential: false });
    setOpen(false);
  }, [map, select, setSettings]);
  const pickWarning = useCallback((f: Feature, keyboard: boolean) => {
    const m = map.current; if (!m) return;
    const at = (f.geometry as Point).coordinates as [number, number];
    select("warning", (f.properties ?? {}) as Props, at, keyboard ? ++picks.current : 0);
    // The card opens where the panel was, as for a search pick.
    setOpen(false);
    m.flyTo({ center: at, zoom: Math.max(m.getZoom(), 6), duration: 1000, essential: false });
  }, [map, select]);
  const centre = () => { const m = map.current; const at = selected ? position(selected.props.lon, selected.props.lat) : null; if (m && at) m.easeTo({ center: at, zoom: Math.max(m.getZoom(), 8) }); };

  const active = settings.ships || settings.aircraft || settings.warnings;
  return <>
    <div className="live-layers">
      {open && <LivePanel settings={settings} setSettings={setSettings} status={status} counts={counts} health={healthById} healthError={health.error ? (health.error instanceof Error ? health.error.message : "request failed") : null} past={past} snapshotAt={snapshotAt} onPast={choose} gap={pastGap} local={local} onPick={pick} onClose={() => setOpen(false)}
        aircraftExtras={settings.aircraft ? <AircraftExtras features={data.current.aircraft} onPick={pick}/> : null}
        tilesFailing={settings.shipSource === "keelwatch" && !!status.ships.error} warningList={warningList} onPickWarning={pickWarning}/>}
      <button ref={toggle} className="globe-style-toggle" aria-label="Layers: ships, aircraft and warnings" aria-expanded={open} onClick={() => setOpen(!open)}><Layers size={14}/><span>Layers</span>{active && <i className={`live-on${past ? " past" : ""}`}/>}</button>
    </div>
    {past && settings.ships && settings.shipSource === "keelwatch" && <div className={`live-past-badge${pastGap ? " gap" : ""}`} role="status"><span>Ships at {when(past)} UTC{pastGap && <small>KeelWatch was not collecting from {when(pastGap.from)} to {when(pastGap.to)}</small>}</span>{chosenAt && <button onClick={() => setChosenAt(null)}>{historical ? "Snapshot time" : "Back to live"}</button>}</div>}
    {past && settings.ships && settings.shipSource === "marinetraffic" && <div className="live-past-badge" role="status"><span>{when(past)} UTC<small>MarineTraffic ships are live only; KeelWatch has the ships at this moment</small></span><button onClick={() => setSettings(s => ({ ...s, shipSource: "keelwatch" }))}>Use KeelWatch</button>{chosenAt && !historical && <button onClick={() => setChosenAt(null)}>Back to live</button>}</div>}
    {hover && <LiveTooltip hover={hover} shipsFetched={data.current.shipsFetched} past={past}/>}
    {selected && <LiveCard selected={selected} shipsFetched={data.current.shipsFetched} track={track} following={following} past={past}
      onTrack={hours => { if (latest.current.selected) void drawTrack(latest.current.selected, hours); }} onFollow={() => setFollowing(f => !f)} onCenter={centre} onClose={clear} onOpenMessage={onSelect}/>}
  </>;
}
