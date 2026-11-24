import { useEffect, useMemo, useState, type ReactNode } from "react";
import type { Feature } from "geojson";
import { useQuery } from "@tanstack/react-query";
import { z } from "zod";
import { Plane, Search, Ship, TriangleAlert, X } from "lucide-react";
import { api } from "../../api";
import { AIRCRAFT_COLOR, AIRCRAFT_GROUPS, SHIP_CATEGORIES, SHIP_COLOR, SHIP_LABEL, type AircraftGroup, type ShipCategory } from "./symbols";
import { ago, altitude, text, type Props } from "./data";
import ShipSourceControl, { type ShipSource } from "./ShipSource";
import { LatestWarnings, WarningWindowControl, windowLabel, type WarningWindow } from "./Warnings";

export type Settings = {
  ships: boolean; aircraft: boolean; warnings: boolean;
  freshness: "30m" | "2h" | "24h" | "7d";
  /** Where ships come from; MarineTraffic is live only. */
  shipSource: ShipSource;
  /** Warnings registered within this many hours of now (or of a past moment). */
  warningHours: WarningWindow;
  /** Warnings MSCIO no longer lists: drawn hollow and dimmed when shown. */
  unlistedWarnings: boolean;
  shipCategories: ShipCategory[]; movement: "all" | "moving" | "still";
  aircraftGroups: AircraftGroup[]; onGround: boolean;
};
export const defaults: Settings = {
  ships: false, aircraft: false, warnings: false, freshness: "24h", shipSource: "keelwatch", warningHours: "720", unlistedWarnings: true,
  shipCategories: [...SHIP_CATEGORIES], movement: "all",
  aircraftGroups: AIRCRAFT_GROUPS.filter(g => g.key !== "ground").map(g => g.key), onGround: true,
};
export type Status = { count: number; total: number | null; mode: "vessels" | "density" | null; acquired: string | null; incomplete: boolean; stale: boolean; error: string | null; loading: boolean; note?: string; military?: number; unlisted?: number; hiddenUnlisted?: number; retained?: number };
/** A layer's provider health from `GET /layers`: "ok", "degraded", "unknown",
 *  "not_configured" or "off" (warnings where Relic does not poll MSCIO). */
export type LayerHealth = { state: string; detail?: string | null; since?: string | null; checked_at?: string | null };
type LayerId = "ships" | "aircraft" | "warnings";
/** Health is also given for the MarineTraffic ship source. */
type HealthId = LayerId | "marinetraffic";
export const idle: Status = { count: 0, total: null, mode: null, acquired: null, incomplete: false, stale: false, error: null, loading: false };
/** A chosen search result; `keyboard` when it was chosen without a pointer. */
export type Pick = { kind: "ship" | "aircraft"; props: Props; keyboard?: boolean };

const listSchema = z.object({ items: z.array(z.record(z.string(), z.any())) }).passthrough();
const identifier = /^[A-Za-z0-9-]{2,12}$/;

function useDebounced(value: string, ms: number) {
  const [v, setV] = useState(value);
  useEffect(() => { const t = window.setTimeout(() => setV(value), ms); return () => window.clearTimeout(t); }, [value, ms]);
  return v;
}

/** Find a ship by name, MMSI or IMO, or a flight by callsign, flight number or
 *  registration: loaded ones instantly, then the providers. */
function Finder({ local, onPick }: { local: (q: string) => { ships: Props[]; aircraft: Props[] }; onPick: (pick: Pick) => void }) {
  const [q, setQ] = useState("");
  const query = useDebounced(q.trim(), 300);
  const ships = useQuery({ queryKey: ["live-find-ships", query], enabled: query.length >= 2, staleTime: 60_000, retry: false,
    queryFn: ({ signal }) => api(`/layers/ships/search?q=${encodeURIComponent(query)}`, listSchema, { signal }) });
  const flights = useQuery({ queryKey: ["live-find-flights", query], enabled: identifier.test(query), staleTime: 20_000, retry: false,
    queryFn: ({ signal }) => api(`/layers/aircraft/search?q=${encodeURIComponent(query)}`, listSchema, { signal }) });
  const found = useMemo((): { ships: Props[]; aircraft: Props[] } => {
    if (query.length < 2) return { ships: [], aircraft: [] };
    const near = local(query);
    const shipIds = new Set(near.ships.map(s => String(s.id)));
    const flightIds = new Set(near.aircraft.map(a => Number(a.flight_id)));
    const remoteShips: Props[] = (ships.data?.items ?? []).filter(s => !shipIds.has(String(s.id))).map(s => ({ ...s, remote: true }));
    const remoteFlights: Props[] = (flights.data?.items ?? []).filter(a => !flightIds.has(Number(a.flight_id))).map(a => ({ ...a, remote: true }));
    return { ships: [...near.ships, ...remoteShips].slice(0, 10), aircraft: [...near.aircraft, ...remoteFlights].slice(0, 10) };
  }, [query, local, ships.data, flights.data]);
  const busy = (ships.isFetching || flights.isFetching) && query.length >= 2;
  const none = query.length >= 2 && !busy && !found.ships.length && !found.aircraft.length;
  return <div className="live-finder">
    {/* Escape first clears the text; only an empty box lets it close the panel. */}
    <label className="live-search"><Search size={14}/><input value={q} onChange={e => setQ(e.target.value)} placeholder="Find a ship or flight" aria-label="Find a ship by name, call sign, MMSI or IMO, or a flight by callsign, flight number or registration" onKeyDown={e => { if (e.key === "Escape" && q) { e.stopPropagation(); setQ(""); } if (e.key === "Enter") { const first = found.ships[0] ? { kind: "ship" as const, props: found.ships[0] } : found.aircraft[0] ? { kind: "aircraft" as const, props: found.aircraft[0] } : null; if (first) onPick({ ...first, keyboard: true }); } }}/>{q && <button aria-label="Clear search" onClick={() => setQ("")}><X size={13}/></button>}</label>
    {query.length >= 2 && <div className="live-results" aria-live="polite">
      {(found.ships.length > 0 || found.aircraft.length > 0) && <ul role="list" aria-label="Matching ships and flights">
        {found.ships.map(s => <li key={`s${s.id}`}><button onClick={e => onPick({ kind: "ship", props: s, keyboard: e.detail === 0 })}>
          <i style={{ background: SHIP_COLOR[(s.category as ShipCategory) ?? "other"] ?? SHIP_COLOR.other }}/>
          <span><strong dir="auto">{text(s.name) ?? "Unnamed vessel"}</strong><small>{[text(s.type), text(s.flag), s.mmsi && `MMSI ${s.mmsi}`, s.imo && `IMO ${s.imo}`].filter(Boolean).join(" · ")}</small></span>
          <em>{s.remote ? ago(text(s.observed_at)) : "in view"}</em>
        </button></li>)}
        {found.aircraft.map(a => <li key={`a${a.flight_id}`}><button onClick={e => onPick({ kind: "aircraft", props: a, keyboard: e.detail === 0 })}>
          <i style={{ background: a.military ? AIRCRAFT_COLOR.military : AIRCRAFT_COLOR.civil }}/>
          <span><strong>{text(a.callsign) ?? text(a.flight_number) ?? "Unknown callsign"}</strong><small>{[text(a.aircraft_type), text(a.registration), (text(a.origin) || text(a.destination)) && `${text(a.origin) ?? "?"} → ${text(a.destination) ?? "?"}`].filter(Boolean).join(" · ")}</small></span>
          <em>{a.remote ? altitude(a.altitude_ft, a.on_ground) : "in view"}</em>
        </button></li>)}
      </ul>}
      {busy && <p className="live-note">Searching KeelWatch{identifier.test(query) ? " and Flightradar24" : ""}…</p>}
      {none && <p className="live-note">No ship or flight matches “{query}”. Ships match by name, call sign, MMSI or IMO. Flights match an exact callsign, flight number or registration.</p>}
      {(ships.error || flights.error) && <p className="live-error">{[ships.error, flights.error].filter(Boolean).map(e => e instanceof Error ? e.message : "Search failed").join(" ")}</p>}
    </div>}
  </div>;
}

const gapTime = (iso: string) => new Date(iso).toLocaleString("en-GB", { day: "numeric", month: "short", hour: "2-digit", minute: "2-digit", timeZone: "UTC" });
/** Live, or a past moment in UTC, in ten-minute steps, with hour steps. */
function TimeControl({ past, snapshotAt, gap, onPast, keelwatch = true }: { past: string | null; snapshotAt: string | null; gap: { from: string; to: string } | null; onPast: (at: string | null) => void; keelwatch?: boolean }) {
  const local = (iso: string) => iso.slice(0, 16);
  const step = (hours: number) => {
    const base = past ? Date.parse(past) : Date.now();
    onPast(new Date(Math.min(Date.now(), base + hours * 3_600_000)).toISOString());
  };
  return <div className="live-time" role="group" aria-label="Time shown">
    <div className="live-segment">
      {!snapshotAt && <button aria-pressed={!past} onClick={() => onPast(null)}>Live</button>}
      {snapshotAt && <button aria-pressed={past === snapshotAt} onClick={() => onPast(null)}>Snapshot time</button>}
      <button aria-pressed={!!past && past !== snapshotAt} onClick={() => step(past ? 0 : -1)}>Past moment</button>
    </div>
    {past && <div className="live-time-pick">
      <button aria-label="One hour earlier" onClick={() => step(-1)}>−1 h</button>
      <label><span className="visually-hidden">Moment in UTC</span><input type="datetime-local" step={600} value={local(past)} max={local(new Date().toISOString())}
        onChange={e => { const t = Date.parse(`${e.target.value}:00Z`); if (Number.isFinite(t)) onPast(new Date(Math.min(Date.now(), t)).toISOString()); }}/><small>UTC</small></label>
      <button aria-label="One hour later" onClick={() => step(1)} disabled={Date.parse(past) > Date.now() - 60_000}>+1 h</button>
    </div>}
    {past && gap && <p className="live-error">KeelWatch was not collecting from {gapTime(gap.from)} to {gapTime(gap.to)} UTC. Ships missing here were not observed, not absent.</p>}
    {past && <p className="live-note">{keelwatch ? "Ships as KeelWatch recorded them: each one's last report in the two hours before this moment, to the nearest ten minutes. Aircraft are live only." : "Past moments need KeelWatch: MarineTraffic ships and aircraft are live only."}</p>}
  </div>;
}

/** Why a layer's provider needs attention (or cannot be judged), under its switch. */
function HealthLine({ id, health }: { id: HealthId; health?: LayerHealth }) {
  if (!health || health.state === "ok" || !health.detail) return null;
  const tone = health.state === "degraded" ? "degraded" : health.state === "unknown" ? "unknown" : "quiet";
  return <p id={`live-health-${id}`} className={`live-health ${tone}`}>{tone !== "quiet" && <TriangleAlert size={11} aria-hidden/>}<span>{health.detail}{health.checked_at && tone !== "quiet" ? <small> · checked {ago(health.checked_at)}</small> : null}</span></p>;
}

export default function LivePanel({ settings, setSettings, status, counts, past, snapshotAt, gap = null, onPast, local, onPick, onClose, health = {}, healthError = null, aircraftExtras = null, tilesFailing = false, warningList = [], onPickWarning }: {
  settings: Settings; setSettings: (update: (s: Settings) => Settings) => void; status: Record<LayerId, Status>; health?: Partial<Record<HealthId, LayerHealth>>; healthError?: string | null;
  /** Live-only aircraft blocks (emergencies, most tracked, who is up), under the aircraft filters. */
  aircraftExtras?: ReactNode;
  /** KeelWatch ship tiles failed in this view. */
  tilesFailing?: boolean;
  /** Warnings in the window, newest first, and choosing one. */
  warningList?: Feature[]; onPickWarning?: (f: Feature, keyboard: boolean) => void;
  counts: { ships: Partial<Record<ShipCategory, number>>; movement: { moving: number; still: number } };
  past: string | null; snapshotAt: string | null; gap?: { from: string; to: string } | null; onPast: (at: string | null) => void;
  local: (q: string) => { ships: Props[]; aircraft: Props[] }; onPick: (pick: Pick) => void; onClose: () => void;
}) {
  const marinetraffic = settings.shipSource === "marinetraffic";
  const toggleCategory = (c: ShipCategory) => setSettings(s => ({ ...s, shipCategories: s.shipCategories.includes(c) ? s.shipCategories.filter(x => x !== c) : [...s.shipCategories, c] }));
  const toggleGroup = (g: AircraftGroup) => setSettings(s => ({ ...s, aircraftGroups: s.aircraftGroups.includes(g) ? s.aircraftGroups.filter(x => x !== g) : [...s.aircraftGroups, g] }));
  // A failure always shows, beside whatever is still drawn from before.
  const failure = (s: Status) => s.error ? <em className="live-error"> · {s.stale ? "stale: " : ""}{s.error}</em> : null;
  const shipLine = (s: Status) => s.error && !s.count ? <span className="live-error">{s.error}</span>
    : s.loading && !s.acquired ? "Loading…"
    : s.note ? s.note
    : marinetraffic ? <>{s.count.toLocaleString("en-GB")} ships in view as MarineTraffic's map shows them · updated {ago(s.acquired)}{s.incomplete && <em> · some areas not received</em>}{failure(s)}</>
    : past ? <>{s.mode === "density" ? "About " : ""}{s.count.toLocaleString("en-GB")} ships recorded in view{failure(s)}</>
    : s.mode === "density" ? <>About {s.count.toLocaleString("en-GB")} ships in view; nearby ships share a dot until you zoom in · updated {ago(s.acquired)}{failure(s)}</>
    : <>{s.count.toLocaleString("en-GB")} ships in view · updated {ago(s.acquired)}{failure(s)}</>;
  const aircraftLine = (s: Status) => s.error && !s.count ? <span className="live-error">{s.error}</span>
    : s.note ? s.note
    : s.loading && !s.acquired ? "Loading…"
    : <>{s.count.toLocaleString("en-GB")} aircraft{s.military ? `, ${s.military} military or government` : ""} · {ago(s.acquired)}{s.incomplete && <em> · some areas at the provider limit</em>}{failure(s)}</>;
  const w = status.warnings;
  // How many shown are no longer listed by MSCIO, or hidden for that reason.
  const unlisted = w.unlisted ? `, ${w.unlisted} no longer listed` : w.hiddenUnlisted ? `; ${w.hiddenUnlisted} no longer listed hidden` : "";
  // The window's count, and how many Relic retains, so a short window's few are never read as all.
  const all = settings.warningHours === "all";
  const retained = w.retained != null && !all ? ` of ${w.retained.toLocaleString("en-GB")} retained` : "";
  const warningLine = w.error && !w.count ? w.error
    : w.loading && !w.acquired ? "Loading…"
    : past ? `${w.count} registered ${all ? "by then" : `in the ${windowLabel(settings.warningHours)} before then`}${retained}${unlisted}${w.error ? ` · ${w.error}` : ""}`
    : `${w.count} ${all ? "retained" : `in the last ${windowLabel(settings.warningHours)}`}${retained}${unlisted} · checked ${ago(w.acquired)}${w.error ? ` · ${w.stale ? "stale: " : ""}${w.error}` : ""}`;
  const described = (id: HealthId) => health[id] && health[id]!.state !== "ok" && health[id]!.detail ? `live-health-${id}` : undefined;
  const shipHealth: HealthId = marinetraffic ? "marinetraffic" : "ships";
  return <section className="live-panel" role="region" aria-label="Live layers">
    <header><h2>Layers</h2><button aria-label="Close layers" onClick={onClose}><X size={15}/></button></header>
    <TimeControl past={past} snapshotAt={snapshotAt} gap={gap} onPast={onPast} keelwatch={!marinetraffic}/>
    <Finder local={local} onPick={onPick}/>

    <div className="live-section">
      <label className="live-switch"><input type="checkbox" role="switch" aria-describedby={described(shipHealth)} checked={settings.ships} onChange={() => setSettings(s => ({ ...s, ships: !s.ships }))}/><Ship size={15}/><span><strong>Ships</strong><small>{marinetraffic ? "MarineTraffic" : `KeelWatch${past ? " history" : ""}`} · {settings.ships ? shipLine(status.ships) : "off"}</small></span></label>
      <HealthLine id={shipHealth} health={health[shipHealth]}/>
      {settings.ships && <>
        <ShipSourceControl source={settings.shipSource} onSource={source => setSettings(s => ({ ...s, shipSource: source }))} keelwatch={health.ships} marinetraffic={health.marinetraffic}
          tilesFailing={tilesFailing} past={past} onLive={snapshotAt ? null : () => onPast(null)}/>
        <div className="live-chips" role="group" aria-label="Ship types">
          {SHIP_CATEGORIES.map(c => <button key={c} aria-pressed={settings.shipCategories.includes(c)} onClick={() => toggleCategory(c)}><i style={{ background: SHIP_COLOR[c] }}/>{SHIP_LABEL[c]}{counts.ships[c] ? <small>{counts.ships[c]!.toLocaleString("en-GB")}</small> : null}</button>)}
        </div>
        <div className="live-row">
          <button className="live-link" onClick={() => setSettings(s => ({ ...s, shipCategories: [...SHIP_CATEGORIES] }))}>All types</button>
          <button className="live-link" onClick={() => setSettings(s => ({ ...s, shipCategories: [] }))}>None</button>
        </div>
        <div className="live-segment" role="group" aria-label="Ship movement">
          {([["all", "All"], ["moving", `Under way${counts.movement.moving ? ` ${counts.movement.moving.toLocaleString("en-GB")}` : ""}`], ["still", `Stopped${counts.movement.still ? ` ${counts.movement.still.toLocaleString("en-GB")}` : ""}`]] as const).map(([k, label]) => <button key={k} aria-pressed={settings.movement === k} onClick={() => setSettings(s => ({ ...s, movement: k }))}>{label}</button>)}
        </div>
        {!past && !marinetraffic && <label className="live-select">Reported within<select value={settings.freshness} onChange={e => setSettings(s => ({ ...s, freshness: e.target.value as Settings["freshness"] }))}><option value="30m">30 minutes</option><option value="2h">2 hours</option><option value="24h">24 hours</option><option value="7d">7 days</option></select></label>}
      </>}
    </div>

    <div className="live-section">
      <label className="live-switch"><input type="checkbox" role="switch" aria-describedby={described("aircraft")} checked={settings.aircraft} onChange={() => setSettings(s => ({ ...s, aircraft: !s.aircraft }))}/><Plane size={15}/><span><strong>Aircraft</strong><small>Flightradar24 · {settings.aircraft ? aircraftLine(status.aircraft) : "off"}</small></span></label>
      <HealthLine id="aircraft" health={health.aircraft}/>
      {settings.aircraft && <>
        <div className="live-chips" role="group" aria-label="Aircraft types">
          {AIRCRAFT_GROUPS.map(g => <button key={g.key} aria-pressed={settings.aircraftGroups.includes(g.key)} onClick={() => toggleGroup(g.key)}><i style={{ background: g.key === "military" ? AIRCRAFT_COLOR.military : g.key === "ground" ? AIRCRAFT_COLOR.ground : AIRCRAFT_COLOR.civil }}/>{g.label}</button>)}
        </div>
        <div className="live-row">
          <button className="live-link" onClick={() => setSettings(s => ({ ...s, aircraftGroups: AIRCRAFT_GROUPS.filter(g => g.key !== "ground").map(g => g.key) }))}>All aircraft</button>
          <button className="live-link" onClick={() => setSettings(s => ({ ...s, aircraftGroups: ["military"] }))}>Only military and government</button>
        </div>
        <label className="live-check"><input type="checkbox" checked={settings.onGround} onChange={() => setSettings(s => ({ ...s, onGround: !s.onGround }))}/>Show aircraft on the ground</label>
        {!past && aircraftExtras}
      </>}
    </div>

    <div className="live-section">
      <label className="live-switch"><input type="checkbox" role="switch" aria-describedby={described("warnings")} checked={settings.warnings} onChange={() => setSettings(s => ({ ...s, warnings: !s.warnings }))}/><TriangleAlert size={15}/><span><strong>Maritime warnings</strong><small>MSCIO · {settings.warnings ? warningLine : "off"}</small></span></label>
      <HealthLine id="warnings" health={health.warnings}/>
      {settings.warnings && <>
        <WarningWindowControl value={settings.warningHours} past={!!past} onChange={h => setSettings(s => ({ ...s, warningHours: h }))}/>
        {onPickWarning && <LatestWarnings items={warningList} now={past ? Date.parse(past) : Date.now()} onPick={onPickWarning}/>}
        <label className="live-check"><input type="checkbox" checked={settings.unlistedWarnings} onChange={() => setSettings(s => ({ ...s, unlistedWarnings: !s.unlistedWarnings }))}/>Show warnings MSCIO no longer lists</label>
        <p className="live-note">MSCIO gives no expiry or cancellation. A warning it no longer lists is drawn hollow and dimmed; that is not a cancellation.</p>
      </>}
    </div>
    {healthError && <p className="live-error">Provider health unavailable: {healthError}</p>}
    <p className="live-note">{marinetraffic && settings.ships ? "Observations as the providers show them" : "Collected observations"}, not every ship or aircraft at sea or in the air.</p>
  </section>;
}
