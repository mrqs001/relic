import { useEffect, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { MOVEMENT_LABEL, SHIP_COLOR, SHIP_LABEL, type Movement, type ShipCategory } from "./symbols";
import { ago, reportAge, shipDetail, text, type Props } from "./data";
import { unlistedText } from "./LiveCard";
import AircraftTooltip from "./AircraftTooltip";
import { warningColorOf } from "./Warnings";

export type Hover = { kind: "ship" | "aircraft" | "warning"; props: Props; x: number; y: number };
/** A past tile's report age (minutes before the moment) in words. */
const pastAge = (minutes: unknown) => {
  const m = minutes == null ? NaN : Math.round(Number(minutes));
  return !Number.isFinite(m) ? "Reported before the chosen time" : m === 0 ? "Reported at the chosen time" : m > 0 ? `Reported ${m} min before the chosen time` : `Reported ${-m} min after the chosen time`;
};

/** A pointer-following preview: enough to decide whether to click. */
export default function LiveTooltip({ hover, shipsFetched, past = null }: { hover: Hover; shipsFetched: string | null; past?: string | null }) {
  // Zoomed-out tiles carry no names: after a short pause, ask KeelWatch.
  // MarineTraffic's satellite contacts have no name to ask for.
  const withheld = hover.kind === "ship" && hover.props.identity_status === "withheld";
  const shipKey = hover.kind === "ship" && !hover.props.name && !withheld ? String(hover.props.key ?? "") : "";
  const [armed, setArmed] = useState(false);
  useEffect(() => { setArmed(false); if (!shipKey) return; const t = window.setTimeout(() => setArmed(true), 180); return () => window.clearTimeout(t); }, [shipKey]);
  const detail = useQuery({ ...shipDetail(shipKey), enabled: armed && !!shipKey });
  const v = (detail.data?.vessel ?? {}) as Props;
  // Identity comes from the current record; at a past moment the voyage and
  // status in it are today's, so only the tile's own (then) values show.
  const p = hover.kind === "ship" ? { ...hover.props, name: hover.props.name ?? v.name, type: hover.props.type ?? v.type, flag: hover.props.flag ?? v.flag, destination: hover.props.destination ?? (past ? null : v.destination), nav_status: hover.props.nav_status ?? (past ? null : v.nav_status) } : hover.props;
  const flip = hover.x > window.innerWidth - 300;
  // Near the bottom edge the preview opens upwards, so it is never cut off.
  const low = hover.y > window.innerHeight - 240;
  const style = { left: flip ? hover.x - 16 : hover.x + 16, top: low ? hover.y - 14 : hover.y + 14, transform: [flip ? "translateX(-100%)" : "", low ? "translateY(-100%)" : ""].join(" ").trim() || undefined };
  if (hover.kind === "warning") {
    const unlisted = unlistedText(p);
    return <div className="live-tooltip" style={style} role="tooltip">
      <span className="live-tip-kind"><i style={unlisted ? { background: "transparent", boxShadow: `inset 0 0 0 1.5px ${warningColorOf(p.type)}` } : { background: warningColorOf(p.type) }}/>Maritime warning{text(p.reference) ? ` · ${p.reference}` : ""}</span>
      <strong>{String(p.type ?? "Warning")}</strong>
      <small>Registered {ago(String(p.registered_at))}</small>
      {unlisted && <small className="live-tip-unlisted">{unlisted}</small>}
      <p dir="auto">{String(p.text ?? "").replace(/\s+/g, " ").slice(0, 150)}…</p>
    </div>;
  }
  if (hover.kind === "ship") {
    const category = (p.category as ShipCategory) ?? "other";
    const speed = p.sog != null ? `${Number(p.sog).toFixed(1)} kn` : null;
    const course = p.cog != null ? `${Math.round(Number(p.cog))}°` : null;
    return <div className="live-tooltip" style={style} role="tooltip">
      <span className="live-tip-kind"><i style={{ background: SHIP_COLOR[category] ?? SHIP_COLOR.other }}/>{text(p.type) ?? SHIP_LABEL[category] ?? "Vessel"}{text(p.flag) ? ` · ${p.flag}` : ""}</span>
      <strong dir="auto">{text(p.name) ?? (withheld ? "Satellite contact, identity withheld" : shipKey && !detail.isFetched ? "…" : "Unnamed vessel")}</strong>
      <span className="live-tip-stats">{text(p.nav_status) ?? MOVEMENT_LABEL[(p.movement as Movement) ?? "unknown"]}{speed && ` · ${speed}`}{course && p.movement === "moving" ? ` · ${course}` : ""}</span>
      {text(p.destination) && <span className="live-tip-route" dir="auto">To {String(p.destination)}</span>}
      <small>{past ? pastAge(p.age_min) : `Reported ${reportAge(p.age_min, shipsFetched)}`}{p.sat ? " · satellite" : ""}</small>
      {Number(p.n) > 1 && <small>One of {Number(p.n).toLocaleString("en-GB")} ships at this spot; zoom in to see each.</small>}
    </div>;
  }
  return <AircraftTooltip p={p} style={style}/>;
}
