import { useEffect, useRef, useState, type CSSProperties, type ReactNode } from "react";
import { useQuery } from "@tanstack/react-query";
import { z } from "zod";
import { Anchor, Check, Crosshair, Eye, LocateFixed, MessageSquareText, Satellite, Siren, X } from "lucide-react";
import { api } from "../../api";
import { ALTITUDE_RAMP, AIRCRAFT_COLOR, MOVEMENT_LABEL, SHIP_COLOR, SHIP_LABEL, SILHOUETTE_LABEL, SPEED_RAMP, flagName, type Movement, type ShipCategory, type Silhouette } from "./symbols";
import { ago, altitude, position, reportAge, shipDetail, text, utc, type Props } from "./data";
import Compass from "./Compass";
import Profile, { type Point } from "./Profile";
import { ROLE_LABEL, describe, estimated, registeredIn, sourceLabel } from "./aircraft";
import { Trend } from "./AircraftTooltip";
import { topFlights } from "./AircraftExtras";
import MarineTrafficCard from "./MarineTrafficCard";
import { warningColorOf } from "./Warnings";

/** `focus` is a token (not 0) when the selection was made from the keyboard:
 *  the card heading takes focus once for each such selection. */
export type Selected = { kind: "ship" | "aircraft" | "warning"; props: Props; inView: boolean; focus?: number };
/** The drawn path behind the selection and its values over time. */
export type Track = { note: string | null; hours: number | null; series: Point[] | null };
const record = z.record(z.string(), z.any());
const names = z.object({ names: z.array(z.object({ name: z.string(), source: z.string().nullish(), first_seen_at: z.string().nullish(), last_seen_at: z.string().nullish() })).nullish() }).passthrough();
const messages = z.object({ items: z.array(z.object({ id: z.string(), revision: z.string(), published_at: z.string().nullish(), preview: z.string(), quoted: z.string().nullish(), state: z.string() })) });

const km = (m: unknown) => Number.isFinite(Number(m)) && Number(m) > 0 ? `${Math.round(Number(m) / 1000).toLocaleString("en-GB")} km` : null;
const clock = (unix: unknown) => Number(unix) > 0 ? new Date(Number(unix) * 1000).toISOString().slice(11, 16) : null;
const date = (iso: unknown) => typeof iso === "string" && iso ? new Date(iso).toLocaleDateString("en-GB", { day: "numeric", month: "short", year: "numeric", timeZone: "UTC" }) : null;
const shortTime = (iso: string) => new Date(iso).toLocaleString("en-GB", { day: "numeric", month: "short", hour: "2-digit", minute: "2-digit", timeZone: "UTC" });

type Common = { selected: Selected; track: Track; following: boolean; past: string | null; onTrack: (hours: number) => void; onFollow: () => void; onCenter: () => void; onClose: () => void; onOpenMessage: (id: string, revision: string) => void };

/** The selected ship, aircraft or warning. */
export default function LiveCard(props: Common & { shipsFetched: string | null }) {
  const { selected } = props;
  if (selected.kind === "warning") return <WarningCard {...props}/>;
  if (selected.kind === "ship" && selected.props.provider === "marinetraffic") return <MarineTrafficCard {...props}/>;
  if (selected.kind === "ship") return <ShipCard {...props}/>;
  return <AircraftCard {...props}/>;
}

/** The card heading, focused after a keyboard pick so reading starts here. */
export function Title({ focus, dir, children }: { focus: number; dir?: "auto"; children: ReactNode }) {
  const ref = useRef<HTMLHeadingElement>(null);
  useEffect(() => { if (focus) ref.current?.focus(); }, [focus]);
  return <h3 ref={ref} tabIndex={-1} dir={dir}>{children}</h3>;
}
export function Close({ onClose }: { onClose: () => void }) {
  return <button className="live-card-close" aria-label="Close" onClick={onClose}><X size={15}/></button>;
}
export function Actions({ selected, following, onFollow, onCenter, past }: Pick<Common, "selected" | "following" | "onFollow" | "onCenter" | "past">) {
  const placed = !!position(selected.props.lon, selected.props.lat);
  return <div className="live-card-actions">
    <button onClick={onCenter} disabled={!placed} title={placed ? undefined : "No reported position to centre on"}><LocateFixed size={13}/>Centre</button>
    {!past && <button aria-pressed={following} onClick={onFollow}><Crosshair size={13}/>{following ? "Following" : "Follow"}</button>}
  </div>;
}

/** MSCIO's listing of a warning, in words; null while it is listed. */
export function unlistedText(p: Props) {
  if (p.listed !== false) return null;
  const since = text(p.unlisted_since);
  return `No longer listed by MSCIO${since ? ` since ${utc(Date.parse(since))}` : ""} — not a cancellation`;
}

function WarningCard({ selected, past, onClose, onOpenMessage }: Common) {
  const p = selected.props;
  const unlisted = unlistedText(p);
  const lastListed = text(p.last_listed_at);
  return <aside className={`live-card${unlisted ? " unlisted" : ""}`} aria-label="Maritime warning" style={{ "--tone": unlisted ? "#9fb3c2" : warningColorOf(p.type) } as CSSProperties}>
    <header className="live-card-head">
      <span className="live-kicker"><i/>Maritime warning · MSCIO{p.reference ? ` · ${String(p.reference)}` : ""}</span>
      <Title focus={selected.focus ?? 0}>{String(p.type || "Warning")}</Title>
      <Close onClose={onClose}/>
    </header>
    {unlisted && <p className="live-unlisted" role="note">{unlisted}<small>{past ? "As recorded at the chosen time. " : ""}MSCIO gives no expiry or cancellation; Relic dates this from the first complete MSCIO list that left it out{lastListed ? `, after it was last listed at ${utc(Date.parse(lastListed))}` : ""}. The retained warning is unchanged.</small></p>}
    <dl className="live-facts">
      <div><dt>Registered</dt><dd>{utc(Date.parse(String(p.registered_at)))}<small>{ago(String(p.registered_at))} · registration time, not necessarily incident time</small></dd></div>
      <div><dt>Position</dt><dd>MSCIO map position<small>MSCIO shifts it by up to about 5 km on each request</small></dd></div>
    </dl>
    <p className="live-warning-text" dir="auto">{String(p.text ?? "")}</p>
    <div className="live-card-actions"><button onClick={() => onOpenMessage(String(p.id), String(p.revision))}><MessageSquareText size={13}/>Open message</button></div>
    <p className="live-note">An official warning while a situation develops, not a verified damage assessment.</p>
  </aside>;
}

function ShipCard({ selected, shipsFetched, track, following, past, onTrack, onFollow, onCenter, onClose, onOpenMessage }: Common & { shipsFetched: string | null }) {
  const p = selected.props;
  const key = String(p.key ?? p.id ?? "");
  const detail = useQuery({ ...shipDetail(key), enabled: !!key });
  const v = (detail.data?.vessel ?? {}) as Props & { particulars?: Props };
  const mmsi = p.mmsi ?? v.mmsi;
  const former = useQuery({ queryKey: ["live-ship-names", key], enabled: !!key, staleTime: 300_000, retry: false,
    queryFn: ({ signal }) => api(`/layers/ships/${encodeURIComponent(key)}/names`, names, { signal }) });
  const named = useQuery({ queryKey: ["live-ship-messages", key, String(mmsi ?? "")], enabled: !!key, staleTime: 30_000, retry: false,
    queryFn: ({ signal }) => api(`/layers/ships/${encodeURIComponent(key)}/messages${mmsi ? `?mmsi=${encodeURIComponent(String(mmsi))}` : ""}`, messages, { signal }) });
  const part = (v.particulars ?? {}) as Record<string, any>;
  const category = ((p.category as ShipCategory) ?? (v.category as ShipCategory) ?? "other");
  const color = SHIP_COLOR[category] ?? SHIP_COLOR.other;
  const name = text(p.name) ?? text(v.name);
  // A past view keeps the position and motion of that moment; particulars are current.
  const speed = p.sog ?? (past ? null : v.speed_knots);
  const course = p.cog ?? (past ? null : v.course);
  const headingValue = p.heading ?? (past ? null : v.heading);
  const heading = headingValue != null && Number(headingValue) < 360 && Number.isFinite(Number(headingValue)) ? Number(headingValue) : null;
  const move = (p.movement as Movement) ?? "unknown";
  const moving = Number(speed ?? 0) >= 0.5;
  const status = text(p.nav_status) ?? (past ? null : text(v.nav_status)) ?? MOVEMENT_LABEL[move];
  // At a past moment only the tile's destination is of that time; ports are the current voyage.
  const destination = text(p.destination) ?? (past ? null : text(v.destination));
  const length = part.length_m ?? p.length_m ?? v.length_m, width = part.width_m ?? p.width_m ?? v.width_m;
  const relative = (minutes: number) => minutes === 0 ? "at the chosen time" : minutes > 0 ? `${minutes} min before the chosen time` : `${-minutes} min after the chosen time`;
  const reported = past && text(p.observed_at)
    ? `${shortTime(String(p.observed_at))} UTC · ${relative(Math.round((Date.parse(past) - Date.parse(String(p.observed_at))) / 60_000))}`
    : past && p.age_min != null ? relative(Math.round(Number(p.age_min)))
    : past ? null
    : p.age_min != null ? reportAge(p.age_min, shipsFetched) : ago(text(p.observed_at) ?? text(v.observed_at));
  const currentName = name?.replace(/\W/g, "").toUpperCase();
  const others = (former.data?.names ?? []).filter(n => n.name.replace(/\W/g, "").toUpperCase() !== currentName);
  const ports = [part.last_port?.name, part.next_port?.name ?? (past ? null : destination)].map(text);
  const identity = [["MMSI", mmsi], ["IMO", p.imo ?? v.imo], ["Call sign", p.callsign ?? v.callsign]].filter(([, x]) => x != null && x !== "") as [string, unknown][];
  // Beam relative to length, drawn at a length scaled to a 400 m hull.
  const hullShare = length && width ? Math.min(1, Number(width) / Number(length) * 3) : null;
  const hullLength = length ? Math.max(0.1, Math.min(1, Number(length) / 400)) : null;
  return <aside className="live-card live-card-ship" aria-label="Vessel" style={{ "--tone": color } as CSSProperties}>
    <header className="live-card-head">
      <span className="live-kicker"><i/>{text(p.type) ?? text(v.type) ?? SHIP_LABEL[category]}{flagName(p.flag ?? v.flag) ? ` · ${flagName(p.flag ?? v.flag)}` : ""}</span>
      <Title focus={selected.focus ?? 0} dir="auto">{name ?? (detail.isLoading ? "Loading…" : "Unnamed vessel")}</Title>
      <span className="live-status"><b data-moving={moving}>{status}</b>{p.sat || v.satellite ? <><Satellite size={11}/>satellite</> : null}</span>
      <Close onClose={onClose}/>
    </header>
    {past && <p className="live-past-note">As recorded at {shortTime(past)} UTC</p>}

    <div className="live-instrument">
      <Compass course={course != null ? Number(course) : null} heading={heading} moving={moving} color={color}/>
      <div className="live-readout">
        <span className="live-big">{speed != null ? Number(speed).toFixed(1) : "—"}<small>kn</small></span>
        <span>{speed == null ? "Speed not reported" : !moving ? "Not under way" : course != null ? `Course ${Math.round(Number(course))}°` : "Course not recorded"}{heading != null ? ` · heading ${heading}°` : ""}</span>
        <span className="live-muted">{reported ? `Reported ${reported}${p.time_estimated ? " (estimated time)" : ""}` : "No report from the chosen time in view; the track shows what was recorded around it"}</span>
        {past && text(p.destination) && <span className="live-muted" dir="auto">Bound for {String(p.destination)} as reported then</span>}
      </div>
    </div>

    {(ports[0] || ports[1]) && <div className="live-voyage">
      <span dir="auto">{ports[0] ?? "Unknown port"}</span>
      <i aria-hidden/>
      <span dir="auto">{ports[1] ?? "Destination not given"}</span>
      {(part.eta || past) && <small>{past ? "Current voyage, not at the chosen time" : ""}{past && part.eta ? " · " : ""}{part.eta ? `ETA ${String(part.eta).replace("T", " ").slice(0, 16)} UTC as broadcast` : ""}</small>}
    </div>}

    {track.series && track.series.length > 1 && <Profile points={track.series} ramp={SPEED_RAMP} gapS={3 * 3600} floor={6} label="Speed along the track" marker={past ? Date.parse(past) / 1000 : null}/>}
    <div className="live-track-choice" role="group" aria-label="Track length">
      <small>Track</small>{[6, 24, 72, 168].map(h => <button key={h} aria-pressed={track.hours === h} onClick={() => onTrack(h)}>{past ? "±" : ""}{h < 48 ? `${h} h` : `${h / 24} d`}</button>)}
    </div>
    {track.note && <p className="live-note">{track.note}</p>}

    <dl className="live-facts">
      {identity.length > 0 && <div><dt>Identity</dt><dd className="live-codes">{identity.map(([k, x]) => <span key={k}><small>{k}</small>{String(x)}</span>)}</dd></div>}
      {(length || width) && <div><dt>Size</dt><dd>
        {length && width ? `${length} × ${width} m` : `${length ?? width} m`}{part.dwt ? ` · ${Number(part.dwt).toLocaleString("en-GB")} dwt` : ""}{part.draught_m ? ` · draught ${part.draught_m} m` : ""}
        {hullShare != null && <span className="live-hull" aria-hidden style={{ "--beam": hullShare, "--length": hullLength } as CSSProperties}><i/><b/></span>}
      </dd></div>}
      {part.year_built && <div><dt>Built</dt><dd>{String(part.year_built)}</dd></div>}
      {v.source && <div><dt>Sources</dt><dd>{String(v.source)}{Number(v.source_count) > 1 ? ` and ${Number(v.source_count) - 1} more` : ""}<small>{String(v.identity_status ?? "identity unknown")}</small></dd></div>}
      {others.length > 0 && <div><dt>Also seen as</dt><dd>{others.slice(0, 4).map(n => <span key={`${n.name}${n.source}`} className="live-former" dir="auto">{n.name}<small>{[n.source, n.last_seen_at && `until ${date(n.last_seen_at)}`].filter(Boolean).join(", ")}</small></span>)}</dd></div>}
    </dl>

    {(named.data?.items.length ?? 0) > 0 && <section className="live-mentions" aria-label="Messages naming this vessel">
      <h4><Anchor size={12}/>Named in {named.data!.items.length} {named.data!.items.length === 1 ? "message" : "messages"}</h4>
      <ul>{named.data!.items.slice(0, 5).map(m => <li key={`${m.id}${m.revision}`}><button onClick={() => onOpenMessage(m.id, m.revision)}>
        <span className="live-mention-meta">{m.state === "confirmed" ? <b><Check size={10}/>Confirmed</b> : <em>Suggested</em>}{m.published_at && <time>{shortTime(m.published_at)}</time>}</span>
        <span className="live-mention-text" dir="auto">{m.preview.replace(/\s+/g, " ").slice(0, 120)}</span>
      </button></li>)}</ul>
    </section>}

    {!selected.inView && !past && <p className="live-note">Not in the current view or filters; showing the last report.</p>}
    {detail.isLoading && <p className="live-note">Loading particulars…</p>}
    <Actions selected={selected} following={following} onFollow={onFollow} onCenter={onCenter} past={past}/>
  </aside>;
}

/** A Mode S address from the flight detail (a number) as six hex digits. */
const hex = (n: unknown) => Number(n) > 0 && Number(n) <= 0xffffff ? Number(n).toString(16).toUpperCase().padStart(6, "0") : null;
/** Climb rate between the last two points of the provider's path. */
function pathFpm(trail: unknown): number | null {
  const t = Array.isArray(trail) ? trail.filter(x => x && Number.isFinite(Number(x.t)) && Number.isFinite(Number(x.altitude_ft))) : [];
  const a = t[t.length - 2], b = t[t.length - 1];
  return a && b && Number(b.t) - Number(a.t) >= 8 && Number(b.t) - Number(a.t) <= 300 ? (Number(b.altitude_ft) - Number(a.altitude_ft)) / ((Number(b.t) - Number(a.t)) / 60) : null;
}

function AircraftCard({ selected, track, following, past, onFollow, onCenter, onClose }: Common) {
  const p = selected.props;
  const flightId = Number(p.flight_id);
  const flight = useQuery({ queryKey: ["live-flight", flightId], enabled: !!flightId, staleTime: 20_000, retry: false,
    queryFn: ({ signal }) => api(`/layers/aircraft/${flightId}`, record, { signal }) });
  // The most-tracked list as the Layers panel last read it; never fetched from here.
  const top = useQuery({ ...topFlights, enabled: false });
  const watched = (top.data?.items ?? []).find(r => Number(r.flight_id) === flightId);
  const [photoFailed, setPhotoFailed] = useState<number | null>(null);
  const f = (flight.data ?? {}) as Record<string, any>;
  const d = describe(p);
  const shape = (p.shape as Silhouette) ?? "narrow";
  const military = p.tone === "military";
  const color = p.emergency ? "#ff5a5a" : military ? AIRCRAFT_COLOR.military : p.tone === "ground" ? AIRCRAFT_COLOR.ground : AIRCRAFT_COLOR.civil;
  const progress = f.progress ?? {};
  const flown = Number(progress.distance_flown_m), remaining = Number(progress.distance_remaining_m);
  const share = flown > 0 && remaining >= 0 ? flown / (flown + remaining) : null;
  const schedule = f.schedule ?? {};
  const photo = !!f.photo && photoFailed !== flightId;
  const callsign = text(p.callsign) ?? text(p.flight_number) ?? text(p.registration) ?? "Unknown callsign";
  const typeName = text(f.aircraft?.type_name) ?? d.typeName ?? text(p.aircraft_type) ?? SILHOUETTE_LABEL[shape];
  // The provider's own airline name first; otherwise what the callsign names.
  const provided = text(f.aircraft?.operator);
  const operator = provided ?? d.operator?.name ?? null;
  const modeS = text(p.icao_hex) ?? hex(f.aircraft?.icao_address);
  const state = registeredIn(modeS);
  const fpm = p.vs_fpm ?? pathFpm(f.trail);
  const source = sourceLabel(p.source);
  const facts = [["Registration", text(p.registration) ?? text(f.aircraft?.registration)], ["Type code", text(p.aircraft_type) ?? text(f.aircraft?.type_code)], ["Mode S", modeS], ["Flight", text(p.flight_number)]].filter(([, x]) => x) as [string, string][];
  const times = [clock(schedule.actual_departure) && `Departed ${clock(schedule.actual_departure)}`, km(progress.distance_remaining_m) && `${km(progress.distance_remaining_m)} to go`, clock(progress.eta) && `arrives ${clock(progress.eta)} UTC`].filter(Boolean).join(" · ");
  return <aside className={`live-card live-card-aircraft${photo ? " with-photo" : ""}`} aria-label="Aircraft" style={{ "--tone": color } as CSSProperties}>
    {photo && <figure className="live-photo"><img src={`/api/v1/layers/aircraft/${flightId}/photo`} alt={`${typeName} ${text(p.registration) ?? ""}`} onError={() => setPhotoFailed(flightId)}/>{text(f.photo?.credit) && <figcaption>Photo {f.photo.credit} · Flightradar24</figcaption>}</figure>}
    <header className="live-card-head">
      <span className="live-kicker"><i/>{military ? "Military or government" : typeName}{operator ? ` · ${operator}` : ""}</span>
      <Title focus={selected.focus ?? 0}>{callsign}</Title>
      {military && <span className="live-status"><b>{typeName}</b>{d.role && <span>{ROLE_LABEL[d.role]}</span>}</span>}
      <Close onClose={onClose}/>
    </header>
    {p.emergency === true && <p className="ac-card-alert" role="note"><Siren size={14} aria-hidden/><span>Emergency status reported by Flightradar24<small>Its schema ties this status to squawk 7500, 7600 or 7700; the free feed gives no squawk code.</small></span></p>}

    {(text(p.origin) || text(p.destination)) && <div className="live-flight-route">
      <strong>{text(p.origin) ?? "?"}</strong>
      <span className="live-progress" role="img" aria-label={share != null ? `${Math.round(share * 100)}% of the route flown` : "Progress unknown"}>
        <i style={{ width: share != null ? `${share * 100}%` : 0 }}/>
        {share != null && <svg viewBox="0 0 16 16" style={{ left: `${share * 100}%` }} aria-hidden><path d="M8 1.5c.6 0 1 .8 1 1.6V6.2l5.5 3.3v1.5L9 9.4v3l1.6 1.2v1.1L8 14l-2.6.7v-1.1L7 12.4v-3l-5.5 1.6V9.5L7 6.2V3.1c0-.8.4-1.6 1-1.6Z" transform="rotate(90 8 8)"/></svg>}
      </span>
      <strong>{text(p.destination) ?? "?"}</strong>
      <small>{times || (flight.isLoading ? "Loading route…" : "Schedule not supplied")}</small>
    </div>}

    <div className="live-stats">
      <div><small>Altitude</small><strong>{altitude(p.altitude_ft, p.on_ground)}</strong>{!p.on_ground && <Trend fpm={fpm}/>}</div>
      <div><small>Ground speed</small><strong>{p.ground_speed_kt != null ? `${p.ground_speed_kt} kn` : "—"}</strong></div>
      <div><small>Track</small><strong>{p.track != null ? `${p.track}°` : "—"}</strong></div>
    </div>
    {track.series && track.series.length > 1 && <Profile points={track.series} ramp={ALTITUDE_RAMP} gapS={15 * 60} floor={10_000} label="Altitude along the path"/>}
    {track.note && <p className="live-note">{track.note}</p>}

    <dl className="live-facts">
      {facts.length > 0 && <div><dt>Aircraft</dt><dd className="live-codes">{facts.map(([k, x]) => <span key={k}><small>{k}</small>{x}</span>)}</dd></div>}
      {operator && <div><dt>Operator</dt><dd>{operator}<small>{provided ? "Flightradar24" : `From the ${d.operator?.basis} prefix ${d.operator?.prefix}`}</small></dd></div>}
      {state && <div><dt>Registered in</dt><dd>{state.name}<small>From the Mode S address</small></dd></div>}
      {military && d.role && <div><dt>Role</dt><dd>{ROLE_LABEL[d.role]}<small>From the aircraft type; the provider lists it as military or government</small></dd></div>}
      <div><dt>Position</dt><dd>{p.observed_at_ms ? `${utc(Number(p.observed_at_ms))}` : "Time unknown"}<small>{[p.observed_at_ms ? ago(Number(p.observed_at_ms)) : null, source && (estimated(p.source) ? "estimated, not measured" : source)].filter(Boolean).join(" · ")}</small></dd></div>
      {watched && <div><dt>Most tracked</dt><dd><span className="ac-watched"><Eye size={11} aria-hidden/>#{String(watched.rank)} · {Number(watched.live_clicks).toLocaleString("en-GB")} watching</span><small>On Flightradar24 now</small></dd></div>}
    </dl>
    {/* The path note already carries the same failure when the detail request failed. */}
    {flight.error && (flight.error instanceof Error ? flight.error.message : "") !== track.note && <p className="live-error">{flight.error instanceof Error ? flight.error.message : "Flight detail unavailable"}</p>}
    {!selected.inView && <p className="live-note">Not in the current view or filters; showing the last report.</p>}
    <Actions selected={selected} following={following} onFollow={onFollow} onCenter={onCenter} past={past}/>
  </aside>;
}
