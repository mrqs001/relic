import { useId } from "react";
import { rampColor, type Ramp } from "./symbols";

export type Point = { t: number; v: number | null };
const W = 300, H = 64, TOP = 6;
const clock = (s: number) => new Date(s * 1000).toISOString().slice(11, 16);
const day = (s: number) => new Date(s * 1000).toLocaleDateString("en-GB", { day: "numeric", month: "short", timeZone: "UTC" });

/** A value over time (altitude along a flight, speed along a track), coloured
 *  by height with the same ramp as the path on the map. Gaps stay open. */
export default function Profile({ points, ramp, gapS, floor, label, marker }: { points: Point[]; ramp: Ramp; gapS: number; floor: number; label: string; marker?: number | null }) {
  const id = useId().replace(/:/g, "");
  const known = points.filter((p): p is { t: number; v: number } => p.v != null && Number.isFinite(p.v)).sort((a, b) => a.t - b.t);
  if (known.length < 2) return null;
  const t0 = known[0].t, t1 = known[known.length - 1].t;
  const top = Math.max(floor, ...known.map(p => p.v));
  const x = (t: number) => t1 > t0 ? (t - t0) / (t1 - t0) * W : 0;
  const y = (v: number) => H - (v / top) * (H - TOP);
  // One run per stretch without a gap; each run is drawn and filled separately.
  const runs: { t: number; v: number }[][] = [];
  for (const p of known) {
    const run = runs[runs.length - 1];
    if (run && p.t - run[run.length - 1].t <= gapS) run.push(p); else runs.push([p]);
  }
  const line = (run: { t: number; v: number }[]) => run.map((p, i) => `${i ? "L" : "M"}${x(p.t).toFixed(1)},${y(p.v).toFixed(1)}`).join("");
  const area = (run: { t: number; v: number }[]) => `${line(run)}L${x(run[run.length - 1].t).toFixed(1)},${H}L${x(run[0].t).toFixed(1)},${H}Z`;
  const peak = known.reduce((a, b) => b.v > a.v ? b : a);
  const last = known[known.length - 1];
  // Ramp stops below the peak, then the peak's own colour (once, even when the peak is a stop).
  const stops = ramp.stops.filter(([v]) => v < top).concat([[top, rampColor(ramp, top)]]);
  const at = marker != null && marker >= t0 && marker <= t1 ? marker : null;
  const sameDay = day(t0) === day(t1);
  return <figure className="live-profile" aria-label={`${label}: highest ${ramp.label(peak.v)} at ${clock(peak.t)} UTC; latest ${ramp.label(last.v)}`}>
    <figcaption><span>{label}</span><strong style={{ color: rampColor(ramp, last.v) }}>{ramp.label(last.v)}</strong></figcaption>
    <svg viewBox={`0 0 ${W} ${H}`} preserveAspectRatio="none" role="img" aria-hidden>
      <defs>
        <linearGradient id={`${id}-v`} gradientUnits="userSpaceOnUse" x1="0" y1={H} x2="0" y2={TOP}>
          {stops.map(([v, c]) => <stop key={v} offset={Math.min(1, v / top)} stopColor={c}/>)}
        </linearGradient>
        {/* The fill fades towards the baseline so the line carries the colour. */}
        <linearGradient id={`${id}-fade`} x1="0" y1="1" x2="0" y2="0"><stop offset="0" stopColor="#fff" stopOpacity="0"/><stop offset="1" stopColor="#fff" stopOpacity="1"/></linearGradient>
        <mask id={`${id}-m`} maskContentUnits="userSpaceOnUse"><rect x="0" y="0" width={W} height={H} fill={`url(#${id}-fade)`}/></mask>
      </defs>
      {[0.25, 0.5, 0.75].map(f => <line key={f} className="live-profile-grid" x1="0" x2={W} y1={y(top * f)} y2={y(top * f)}/>)}
      {runs.map((run, i) => <g key={i}>
        <path d={area(run)} fill={`url(#${id}-v)`} opacity={0.32} mask={`url(#${id}-m)`}/>
        <path d={line(run)} fill="none" stroke={`url(#${id}-v)`} strokeWidth={1.8} vectorEffect="non-scaling-stroke" strokeLinejoin="round"/>
      </g>)}
      {at != null && <line className="live-profile-marker" x1={x(at)} x2={x(at)} y1={TOP - 4} y2={H} vectorEffect="non-scaling-stroke"/>}
    </svg>
    <div className="live-profile-axis"><span>{sameDay ? clock(t0) : `${day(t0)} ${clock(t0)}`}</span><span>peak {ramp.label(peak.v)}</span><span>{sameDay ? clock(t1) : `${day(t1)} ${clock(t1)}`} UTC</span></div>
  </figure>;
}
