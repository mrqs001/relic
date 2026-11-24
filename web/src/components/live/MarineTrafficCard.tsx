import { useState, type CSSProperties } from "react";
import { useQuery } from "@tanstack/react-query";
import { z } from "zod";
import { Anchor, Check, Satellite } from "lucide-react";
import { api } from "../../api";
import { MOVEMENT_LABEL, SHIP_COLOR, SHIP_LABEL, flagName, movement, type ShipCategory } from "./symbols";
import { ago, reportAge, shipDetail, text, type Props } from "./data";
import Compass from "./Compass";
import { Actions, Close, Title, type Selected } from "./LiveCard";

const messages = z.object({ items: z.array(z.object({ id: z.string(), revision: z.string(), published_at: z.string().nullish(), preview: z.string(), state: z.string() })) });
const shortTime = (iso: string) => new Date(iso).toLocaleString("en-GB", { day: "numeric", month: "short", hour: "2-digit", minute: "2-digit", timeZone: "UTC" });
/** MarineTraffic sends port names in capitals; shown in title case for reading. */
const titleCase = (s: string) => s.toLowerCase().replace(/(^|[\s(/-])(\p{L})/gu, (_, a: string, b: string) => a + b.toUpperCase());
const num = (v: unknown) => typeof v === "number" && Number.isFinite(v) ? v : null;
type Port = { name?: string | null; country?: string | null; unlocode?: string | null } | null | undefined;

/** A ship from MarineTraffic's live map: its info window and particulars,
 *  the voyage between ports, condition and identity. No track: Relic keeps
 *  no MarineTraffic history. Satellite contacts keep their withheld identity. */
export default function MarineTrafficCard({ selected, shipsFetched, following, onFollow, onCenter, onClose, onOpenMessage }: {
  selected: Selected; shipsFetched: string | null; following: boolean; onFollow: () => void; onCenter: () => void; onClose: () => void; onOpenMessage: (id: string, revision: string) => void;
}) {
  const p = selected.props;
  const key = String(p.key ?? p.id ?? "");
  const withheld = p.sat === true && !p.ship_id;
  const detail = useQuery({ ...shipDetail(key), enabled: !!key && !withheld, retry: false });
  const v = (detail.data?.vessel ?? {}) as Props & { particulars?: Record<string, unknown> };
  const part = (v.particulars ?? {}) as Record<string, unknown>;
  const mmsi = v.mmsi ?? p.mmsi;
  const named = useQuery({ queryKey: ["live-ship-messages", key, String(mmsi ?? "")], enabled: !!key && mmsi != null, staleTime: 30_000, retry: false,
    queryFn: ({ signal }) => api(`/layers/ships/${encodeURIComponent(key)}/messages?mmsi=${encodeURIComponent(String(mmsi))}`, messages, { signal }) });
  const [photoFailed, setPhotoFailed] = useState<string | null>(null);
  const category = ((p.category as ShipCategory) ?? (v.category as ShipCategory) ?? "other");
  const color = SHIP_COLOR[category] ?? SHIP_COLOR.other;
  const name = text(p.name) ?? text(v.name);
  // The map's report is the one drawn; the info window may be a little newer.
  const speed = num(p.sog) ?? num(v.speed_knots);
  const course = num(p.cog) ?? num(v.course);
  const heading = num(p.heading);
  const status = text(v.nav_status) ?? text(p.nav_status);
  const move = movement(status, speed);
  const moving = Number(speed ?? 0) >= 0.5;
  const reported = text(v.observed_at) ? ago(String(v.observed_at)) : p.age_min != null ? reportAge(p.age_min, shipsFetched) : null;
  const shipId = String(p.ship_id ?? v.ship_id ?? "");
  const photo = !withheld && !!shipId && Number(v.photos ?? 0) > 0 && photoFailed !== shipId;
  const from = part.last_port as Port, to = part.next_port as Port;
  const destination = text(v.destination) ?? text(p.destination);
  const progress = from && to && num(part.voyage_progress) != null ? Math.max(0, Math.min(100, Number(part.voyage_progress))) : null;
  const length = num(v.length_m) ?? num(p.length_m), width = num(v.width_m) ?? num(p.width_m);
  const hullShare = length && width ? Math.min(1, width / length * 3) : null;
  const hullLength = length ? Math.max(0.1, Math.min(1, length / 400)) : null;
  const identity = [["MMSI", mmsi], ["IMO", v.imo], ["Call sign", v.callsign]].filter(([, x]) => x != null && x !== "") as [string, unknown][];
  const kind = [text(part.market), text(part.size_class), text(part.ais_class)].filter(Boolean).map(s => titleCase(String(s))).join(" · ");
  const draught = num(part.draught_m), maxDraught = num(part.max_draught_m), dwt = num(part.dwt) ?? num(p.dwt);
  const voyageTimes = [text(part.departed_at) && `Departed ${shortTime(String(part.departed_at))}`, text(part.eta) && `ETA ${shortTime(String(part.eta))} UTC as broadcast`].filter(Boolean).join(" · ");
  const typeLabel = text(v.type) ?? text(p.type) ?? SHIP_LABEL[category];
  return <aside className={`live-card live-card-ship${photo ? " with-photo" : ""}`} aria-label="Vessel, MarineTraffic" style={{ "--tone": color } as CSSProperties}>
    {photo && <figure className="live-photo"><img src={`/api/v1/layers/ships/marinetraffic/${encodeURIComponent(shipId)}/photo`} alt={name ?? "Vessel"} onError={() => setPhotoFailed(shipId)}/><figcaption>Photo · MarineTraffic</figcaption></figure>}
    <header className="live-card-head">
      <span className="live-kicker"><i/>{typeLabel}{flagName(v.flag ?? p.flag) ? ` · ${flagName(v.flag ?? p.flag)}` : ""}<span className="live-provider">MarineTraffic</span></span>
      <Title focus={selected.focus ?? 0} dir="auto">{name ?? (withheld ? "Satellite contact" : detail.isLoading ? "Loading…" : "Unnamed vessel")}</Title>
      <span className="live-status"><b data-moving={moving}>{status ?? MOVEMENT_LABEL[move]}</b>{p.sat ? <><Satellite size={11}/>satellite</> : null}</span>
      <Close onClose={onClose}/>
    </header>

    <div className="live-instrument">
      <Compass course={course} heading={heading} moving={moving} color={color}/>
      <div className="live-readout">
        <span className="live-big">{speed != null ? speed.toFixed(1) : "—"}<small>kn</small></span>
        <span>{speed == null ? "Speed not reported" : !moving ? "Not under way" : course != null ? `Course ${Math.round(course)}°` : "Course not reported"}{heading != null ? ` · heading ${Math.round(heading)}°` : ""}</span>
        <span className="live-muted">{reported ? `Reported ${reported}` : "Report time unknown"}{text(v.ais_source) ? ` · ${String(v.ais_source).toLowerCase()} AIS` : ""}</span>
      </div>
    </div>

    {(from || to) ? <div className="live-flight-route live-sea-route">
      <strong title={text(from?.name) ?? undefined}>{text(from?.unlocode) ?? "—"}</strong>
      <span className="live-progress" role="img" aria-label={progress != null ? `MarineTraffic shows the voyage ${Math.round(progress)}% done` : "Voyage progress unknown"}>
        <i style={{ width: progress != null ? `${progress}%` : 0 }}/>
        {progress != null && <svg viewBox="0 0 16 16" style={{ left: `${progress}%` }} aria-hidden><path d="M8 1c2.2 2 3.2 4.3 3.2 7.2L11 15H5l-.2-6.8C4.8 5.3 5.8 3 8 1Z" transform="rotate(90 8 8)"/></svg>}
      </span>
      <strong title={text(to?.name) ?? undefined}>{text(to?.unlocode) ?? (destination ? "…" : "—")}</strong>
      <span className="live-ports"><span dir="auto">{text(from?.name) ? titleCase(String(from!.name)) : "Last port unknown"}</span><span dir="auto">{text(to?.name) ? titleCase(String(to!.name)) : destination ? `To ${destination}` : "Next port not given"}</span></span>
      {voyageTimes && <small>{voyageTimes}</small>}
    </div> : destination ? <div className="live-voyage"><span>Bound for</span><i aria-hidden/><span dir="auto">{destination}</span><small>Destination as broadcast</small></div> : null}

    {(draught != null || text(v.load_condition) || dwt != null) && <div className="live-stats">
      <div><small>Draught</small><strong>{draught != null ? `${draught.toFixed(1)} m` : "—"}</strong>{maxDraught != null && <small>max {maxDraught} m</small>}</div>
      <div><small>Load</small><strong>{text(v.load_condition) ?? "—"}</strong></div>
      <div><small>Deadweight</small><strong>{dwt != null ? `${dwt.toLocaleString("en-GB")} t` : "—"}</strong></div>
    </div>}

    <dl className="live-facts">
      {identity.length > 0 && <div><dt>Identity</dt><dd className="live-codes">{identity.map(([k, x]) => <span key={k}><small>{k}</small>{String(x)}</span>)}</dd></div>}
      {(length || width) && <div><dt>Size</dt><dd>
        {length && width ? `${length} × ${width} m` : `${length ?? width} m`}
        {hullShare != null && <span className="live-hull" aria-hidden style={{ "--beam": hullShare, "--length": hullLength } as CSSProperties}><i/><b/></span>}
      </dd></div>}
      {kind && <div><dt>Class</dt><dd>{kind}</dd></div>}
      {num(part.year_built) != null && <div><dt>Built</dt><dd>{String(part.year_built)}</dd></div>}
      {withheld && <div><dt>Identity</dt><dd>Withheld<small>MarineTraffic gives satellite contacts no name or identifiers</small></dd></div>}
    </dl>

    {(named.data?.items.length ?? 0) > 0 && <section className="live-mentions" aria-label="Messages naming this vessel">
      <h4><Anchor size={12}/>Named in {named.data!.items.length} {named.data!.items.length === 1 ? "message" : "messages"}</h4>
      <ul>{named.data!.items.slice(0, 5).map(m => <li key={`${m.id}${m.revision}`}><button onClick={() => onOpenMessage(m.id, m.revision)}>
        <span className="live-mention-meta">{m.state === "confirmed" ? <b><Check size={10}/>Confirmed</b> : <em>Suggested</em>}{m.published_at && <time>{shortTime(m.published_at)}</time>}</span>
        <span className="live-mention-text" dir="auto">{m.preview.replace(/\s+/g, " ").slice(0, 120)}</span>
      </button></li>)}</ul>
    </section>}

    {detail.error && <p className="live-error">{detail.error instanceof Error ? detail.error.message : "MarineTraffic detail unavailable"}</p>}
    {detail.data?.stale === true && <p className="live-error">Detail from {ago(String(detail.data.acquired_at ?? ""))}: {String(detail.data.error ?? "MarineTraffic did not answer")}</p>}
    {!selected.inView && <p className="live-note">Not in the current view or filters; showing the last report.</p>}
    <p className="live-note">Live from MarineTraffic's public map, through Relic's proxies. Relic keeps no MarineTraffic history, so there is no track; collected history is KeelWatch's.</p>
    <Actions selected={selected} following={following} onFollow={onFollow} onCenter={onCenter} past={null}/>
  </aside>;
}
