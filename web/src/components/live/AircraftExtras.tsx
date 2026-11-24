import { useMemo, useState } from "react";
import type { Feature } from "geojson";
import { useQuery } from "@tanstack/react-query";
import { z } from "zod";
import { ChevronDown, Eye, Shield, Siren } from "lucide-react";
import { api } from "../../api";
import { AIRCRAFT_COLOR } from "./symbols";
import { ago, altitude, text, type Props } from "./data";
import { aircraftName, aircraftProps, describe, militaryPicture, roleCount, typeName } from "./aircraft";

type Pick = { kind: "aircraft"; props: Props; keyboard?: boolean };
const topSchema = z.object({
  items: z.array(z.record(z.string(), z.any())).default([]),
  meta: z.record(z.string(), z.any()).nullish(),
  acquired_at: z.string().nullish(), stale: z.boolean().nullish(), error: z.string().nullish(),
}).passthrough();
/** Flightradar24's most-tracked list through Relic; the card reads the same cache. */
export const topFlights = {
  queryKey: ["live-top-flights"],
  staleTime: 30_000,
  queryFn: ({ signal }: { signal: AbortSignal }) => api("/layers/aircraft/top", topSchema, { signal }),
};
const compact = new Intl.NumberFormat("en-GB", { notation: "compact", maximumFractionDigits: 1 });
const route = (p: Props) => {
  const from = text(p.origin) ?? text(p.origin_city), to = text(p.destination) ?? text(p.destination_city);
  return from || to ? `${from ?? "?"} → ${to ?? "?"}` : null;
};

/** The flights most watched on Flightradar24 right now; choosing one selects
 *  it on the map. Asked for only while the panel shows it, once a minute. */
function MostTracked({ inView, onPick }: { inView: Map<number, Props>; onPick: (pick: Pick) => void }) {
  const [all, setAll] = useState(false);
  const top = useQuery({ ...topFlights, refetchInterval: 60_000, retry: false });
  const rows = (top.data?.items ?? []) as Props[];
  const positions = top.data?.meta?.positions_error as string | undefined;
  return <section className="ac-block" aria-label="Most tracked flights on Flightradar24">
    <header>
      <h3><Eye size={12} aria-hidden/>Most tracked now</h3>
      <small>{top.data ? `Flightradar24 · ${ago(top.data.acquired_at)}` : top.isLoading ? "Loading…" : ""}</small>
    </header>
    {top.error && !top.data && <p className="live-error">Most-tracked list unavailable: {top.error instanceof Error ? top.error.message : "request failed"}</p>}
    {top.data?.stale && <p className="live-error">Stale list: {top.data.error}</p>}
    {positions && <p className="live-error">{positions}. Flights without a position cannot be shown.</p>}
    {top.data && !rows.length && <p className="live-note">Flightradar24 lists no flights now.</p>}
    {rows.length > 0 && <ol className="ac-top">
      {rows.slice(0, all ? rows.length : 5).map(r => {
        const here = inView.get(Number(r.flight_id));
        const p = aircraftProps({ ...r, ...(here ?? {}) });
        const placed = r.positioned === true || !!here;
        const tone = p.emergency ? "#ff5a5a" : p.tone === "military" ? AIRCRAFT_COLOR.military : AIRCRAFT_COLOR.civil;
        const what = text(r.description) ?? typeName(r.aircraft_type) ?? text(r.aircraft_type);
        return <li key={String(r.flight_id)}>
          <button disabled={!placed} title={placed ? `Show on the map${here ? " (in view)" : ""}` : "Flightradar24 gave no current position"} onClick={e => onPick({ kind: "aircraft", props: p, keyboard: e.detail === 0 })}>
            <span className="ac-rank" style={{ color: tone }}>{String(r.rank)}</span>
            <span className="ac-top-main">
              <strong>{aircraftName(r)}{here && <em>in view</em>}</strong>
              <small>{[route(r), what, !placed ? "no position" : null].filter(Boolean).join(" · ")}</small>
            </span>
            <span className="ac-watch" title={`${Number(r.live_clicks).toLocaleString("en-GB")} watching on Flightradar24 now`}><Eye size={10} aria-hidden/>{compact.format(Number(r.live_clicks) || 0)}</span>
          </button>
        </li>;
      })}
    </ol>}
    {rows.length > 5 && <button className="live-link ac-more" onClick={() => setAll(!all)}>{all ? "Show fewer" : `Show all ${rows.length}`}</button>}
    {rows.length > 0 && <p className="live-note">Ranked by people watching on Flightradar24: attention, not importance.</p>}
  </section>;
}

/** Aircraft whose feed status is an emergency, first and impossible to miss. */
function Emergencies({ aircraft, onPick }: { aircraft: Props[]; onPick: (pick: Pick) => void }) {
  if (!aircraft.length) return null;
  return <section className="ac-block ac-alerts" aria-label="Aircraft reporting an emergency">
    <header><h3><Siren size={12} aria-hidden/>{aircraft.length === 1 ? "1 aircraft reports an emergency" : `${aircraft.length} aircraft report an emergency`}</h3></header>
    <ul>{aircraft.map(p => <li key={String(p.flight_id)}><button onClick={e => onPick({ kind: "aircraft", props: p, keyboard: e.detail === 0 })}>
      <strong>{aircraftName(p)}</strong><small>{[describe(p).typeName ?? text(p.aircraft_type), altitude(p.altitude_ft, p.on_ground)].filter(Boolean).join(" · ")}</small>
    </button></li>)}</ul>
    <p className="live-note">The provider's emergency status; Flightradar24's free feed gives no squawk code.</p>
  </section>;
}

/** Military and government aircraft in view, by the state their callsign or
 *  Mode S address names, each row opening its aircraft. */
function StatePicture({ features, onPick }: { features: Feature[]; onPick: (pick: Pick) => void }) {
  const picture = useMemo(() => militaryPicture(features), [features]);
  const [open, setOpen] = useState<string | null>(null);
  const [more, setMore] = useState(false);
  if (!picture.total) return null;
  const rows = more ? picture.rows : picture.rows.slice(0, 6);
  return <section className="ac-block" aria-label="Military and government aircraft in view">
    <header><h3><Shield size={12} aria-hidden/>Military and government in view</h3><b className="ac-total">{picture.total}</b></header>
    <ul className="ac-states">{rows.map(r => {
      const key = r.code || "none";
      const expanded = open === key;
      return <li key={key} className={expanded ? "open" : undefined}>
        <button aria-expanded={expanded} onClick={() => setOpen(expanded ? null : key)}>
          <i className={`ac-cc${r.code ? "" : " none"}`}>{r.code || "?"}</i>
          <span className="ac-state-main"><strong>{r.name}</strong><small>{r.roles.length ? r.roles.slice(0, 3).map(([role, n]) => roleCount(role, n)).join(" · ") : "role not known from the type"}</small></span>
          <b>{r.count}</b>
          <ChevronDown size={12} aria-hidden className="ac-chevron"/>
        </button>
        {expanded && <ul className="ac-state-list">{r.aircraft.slice(0, 24).map(p => <li key={String(p.flight_id)}><button onClick={e => onPick({ kind: "aircraft", props: p, keyboard: e.detail === 0 })}>
          <strong>{aircraftName(p)}</strong><small>{[typeName(p.aircraft_type) ?? text(p.aircraft_type), altitude(p.altitude_ft, p.on_ground)].filter(Boolean).join(" · ")}</small>
        </button></li>)}{r.aircraft.length > 24 && <li className="live-note">and {r.aircraft.length - 24} more</li>}</ul>}
      </li>;
    })}</ul>
    {picture.rows.length > 6 && <button className="live-link ac-more" onClick={() => setMore(!more)}>{more ? "Show fewer" : `Show all ${picture.rows.length} states`}</button>}
    <p className="live-note">Flightradar24's military and government category in this view now, grouped by callsign or Mode S registration. One frame, no baseline.</p>
  </section>;
}

/** The aircraft section's extras in the Layers panel. */
export default function AircraftExtras({ features, onPick }: { features: Feature[]; onPick: (pick: Pick) => void }) {
  const props = useMemo(() => features.map(f => (f.properties ?? {}) as Props), [features]);
  const inView = useMemo(() => new Map(props.map(p => [Number(p.flight_id), p])), [props]);
  const alerts = useMemo(() => props.filter(p => p.emergency === true), [props]);
  return <>
    <Emergencies aircraft={alerts} onPick={onPick}/>
    <MostTracked inView={inView} onPick={onPick}/>
    <StatePicture features={features} onPick={onPick}/>
  </>;
}
