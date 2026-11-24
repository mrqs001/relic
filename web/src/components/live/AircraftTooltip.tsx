import type { CSSProperties } from "react";
import { MoveDownRight, MoveRight, MoveUpRight, Siren } from "lucide-react";
import { AIRCRAFT_COLOR } from "./symbols";
import { ago, altitude, finite, text, type Props } from "./data";
import { ROLE_LABEL, aircraftName, climb, describe, estimated, fpmText, shapeLabel } from "./aircraft";

/** Climb, descent or level flight between the last two reports, as an arrow. */
export function Trend({ fpm, size = 11 }: { fpm: unknown; size?: number }) {
  const c = climb(fpm);
  if (!c) return null;
  const Icon = c === "climbing" ? MoveUpRight : c === "descending" ? MoveDownRight : MoveRight;
  return <span className={`ac-trend ${c}`} title={`${fpmText(fpm)} between the last two reports Relic received`}><Icon size={size} aria-hidden/>{c === "level" ? "level" : fpmText(fpm)}</span>;
}

/** The aircraft under the pointer: who flies it, what it is, where it is
 *  going and how it is moving, from the live feed and Relic's own tables. */
export default function AircraftTooltip({ p, style }: { p: Props; style: CSSProperties }) {
  const d = describe(p);
  const military = p.tone === "military";
  const color = p.emergency ? "#ff5a5a" : military ? AIRCRAFT_COLOR.military : p.tone === "ground" ? AIRCRAFT_COLOR.ground : AIRCRAFT_COLOR.civil;
  const name = aircraftName(p);
  const flight = text(p.flight_number) && text(p.flight_number) !== text(p.callsign) ? text(p.flight_number) : null;
  const type = text(p.aircraft_type);
  const who = d.operator?.name ?? (military ? d.country?.name : null) ?? shapeLabel(p);
  const origin = text(p.origin), destination = text(p.destination);
  const speed = finite(p.ground_speed_kt), track = finite(p.track);
  const observed = finite(p.observed_at_ms);
  return <div className={`live-tooltip ac-tip${military ? " state" : ""}${p.emergency ? " alert" : ""}`} style={{ ...style, "--tone": color } as CSSProperties} role="tooltip">
    {p.emergency === true && <span className="ac-tip-alert"><Siren size={11} aria-hidden/>Emergency status reported by Flightradar24</span>}
    <span className="ac-tip-who">
      {military && <b className="ac-badge">Military · government</b>}
      <span>{who}</span>
      {d.country && <i className="ac-cc" title={`Mode S address registered to ${d.country.name}`}>{d.country.code}</i>}
    </span>
    <span className="ac-tip-title"><strong>{name}</strong>{flight && <small>{flight}</small>}{text(p.registration) && text(p.registration) !== name && <em>{text(p.registration)}</em>}</span>
    <span className="ac-tip-type">
      <span>{d.typeName ?? type ?? shapeLabel(p)}</span>
      {d.typeName && type && <small>{type}</small>}
      {d.role && <span className="ac-role">{ROLE_LABEL[d.role]}</span>}
    </span>
    {(origin || destination) && <span className="ac-tip-route"><span>{origin ?? "?"}</span><i aria-hidden/><span>{destination ?? "?"}</span></span>}
    <span className="ac-tip-metrics">
      <span><small>Altitude</small><strong>{altitude(p.altitude_ft, p.on_ground)}</strong>{!p.on_ground && <Trend fpm={p.vs_fpm}/>}</span>
      <span><small>Ground speed</small><strong>{speed != null ? `${speed} kn` : "—"}</strong></span>
      <span><small>Heading</small><strong>{track != null ? `${track}°` : "—"}</strong></span>
    </span>
    <small className={`ac-tip-foot${estimated(p.source) ? " estimated" : ""}`}>{[d.source && (estimated(p.source) ? "Estimated position" : d.source), observed ? ago(observed) : "time unknown"].filter(Boolean).join(" · ")}</small>
  </div>;
}
