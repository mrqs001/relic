import { useEffect, useRef, useState } from "react";
import { ChevronLeft, ChevronRight, Clock3, Pause, Play, X } from "lucide-react";
import type { Scope, Timeline as TimelineData } from "../api";
import { shiftInterval, timeWindows } from "../time-window";
import type { TimeWindow } from "../time-window";
import "./timeline.css";

const stamp = (time: string) => new Date(time).toLocaleString("en-GB", { timeZone: "UTC", day: "numeric", month: "short", hour: "2-digit", minute: "2-digit", hourCycle: "h23" });
type Props = {
  window: TimeWindow | null; scope: Scope; data?: TimelineData; busy: boolean;
  onWindow: (window: TimeWindow | null) => void;
  onInterval: (from: string, to: string) => void;
  onBasis: (basis: Scope["time_basis"]) => void;
};
export default function Timeline({ window, scope, data, busy, onWindow, onInterval, onBasis }: Props) {
  const [custom, setCustom] = useState(false), [focused, setFocused] = useState(0),
    [selection, setSelection] = useState<[number, number] | null>(null);
  const strip = useRef<HTMLDivElement>(null), options = useRef<HTMLDivElement>(null),
    dateButton = useRef<HTMLButtonElement>(null), drag = useRef<number | null>(null),
    lastWindow = useRef<TimeWindow>(window ?? "24h");
  if (window) lastWindow.current = window;
  const from = scope.from ?? data?.from, to = scope.to ?? data?.to;
  const buckets = data?.buckets ?? [], maximum = Math.max(1, ...buckets.map(b => b.count));
  const fixed = !window && !!(scope.from || scope.to);
  useEffect(() => { setSelection(null); drag.current = null; }, [data, busy]);
  useEffect(() => {
    if (!custom) return;
    const close = (e: PointerEvent) => { if (!options.current?.contains(e.target as Node) && !dateButton.current?.contains(e.target as Node)) setCustom(false); };
    const escape = (e: KeyboardEvent) => { if (e.key === "Escape") { e.stopPropagation(); setCustom(false); dateButton.current?.focus(); } };
    document.addEventListener("pointerdown", close);
    document.addEventListener("keydown", escape, true);
    return () => { document.removeEventListener("pointerdown", close); document.removeEventListener("keydown", escape, true); };
  }, [custom]);
  const select = (a: number, b = a) => {
    if (busy || !buckets.length) return;
    onInterval(buckets[Math.min(a, b)].from, buckets[Math.max(a, b)].to);
    setSelection(null);
  };
  const indexAt = (clientX: number) => {
    const rect = strip.current!.getBoundingClientRect();
    return Math.max(0, Math.min(buckets.length - 1, Math.floor((clientX - rect.left) / rect.width * buckets.length)));
  };
  const step = (direction: -1 | 1) => {
    if (from && to) { const next = shiftInterval(from, to, direction); onInterval(next.from, next.to); }
  };
  return <section className="map-timeline" aria-label="Message timeline">
    <div className="timeline-controls">
      <div className="time-presets" aria-label="Time window">
        {(Object.keys(timeWindows) as TimeWindow[]).map(preset => <button key={preset} aria-label={`Last ${preset}`} aria-pressed={window === preset} onClick={() => onWindow(preset)}>{preset}</button>)}
        <button aria-label="All retained time" aria-pressed={!window && !fixed} onClick={() => onWindow(null)}>All</button>
      </div>
      <div className="time-navigation">
        <button aria-label="Previous time interval" title="Previous interval" disabled={!from || !to || busy} onClick={() => step(-1)}><ChevronLeft size={14}/></button>
        <button aria-label="Next time interval" title="Next interval" disabled={!fixed || !from || !to || busy} onClick={() => step(1)}><ChevronRight size={14}/></button>
        {(window || fixed) && <button className={`time-live ${window ? "is-live" : ""}`} aria-label={window ? "Pause time window" : `Resume last ${lastWindow.current}`} title={window ? "Hold this time interval; messages still update" : `Return to last ${lastWindow.current}`} onClick={() => window && from && to ? onInterval(from, to) : onWindow(lastWindow.current)}>{window ? <Pause size={11}/> : <Play size={11}/>}<span>{window ? "Live" : "Paused"}</span></button>}
      </div>
    </div>
    <div className="timeline-chart" ref={strip} aria-label="Message activity; drag to select an interval" aria-busy={busy}
      onPointerDown={e => { if (busy || !buckets.length || e.button !== 0) return; drag.current = indexAt(e.clientX); setSelection([drag.current, drag.current]); e.currentTarget.setPointerCapture(e.pointerId); }}
      onPointerMove={e => { if (drag.current !== null) setSelection([drag.current, indexAt(e.clientX)]); }}
      onPointerUp={e => { if (drag.current !== null) { select(drag.current, indexAt(e.clientX)); drag.current = null; e.currentTarget.releasePointerCapture(e.pointerId); } }}
      onPointerCancel={() => { drag.current = null; setSelection(null); }}>
      {buckets.map((bucket, i) => <button key={i} disabled={busy} tabIndex={i === Math.min(focused, buckets.length - 1) ? 0 : -1}
        className={selection && i >= Math.min(...selection) && i <= Math.max(...selection) ? "in-selection" : ""}
        aria-label={`${bucket.count} messages, ${stamp(bucket.from)} to ${stamp(bucket.to)} UTC; select interval`}
        title={`${bucket.count} messages · ${stamp(bucket.from)}–${stamp(bucket.to)} UTC. Click or drag to select.`}
        onFocus={() => setFocused(i)} onClick={e => { if (e.detail === 0) select(i); }}
        onKeyDown={e => { if (["ArrowLeft", "ArrowRight", "Home", "End"].includes(e.key)) {
          e.preventDefault();
          const next = e.key === "Home" ? 0 : e.key === "End" ? buckets.length - 1 : Math.max(0, Math.min(buckets.length - 1, i + (e.key === "ArrowRight" ? 1 : -1)));
          (strip.current?.children[next] as HTMLElement)?.focus();
        } }}><span style={{ height: `${bucket.count ? Math.max(8, bucket.count / maximum * 100) : 0}%` }}/></button>)}
      {!buckets.some(b => b.count) ? <span className="timeline-empty">{busy ? "Updating…" : "No dated messages in this period"}</span> : null}
    </div>
    <div className="timeline-axis">
      <span>{data?.earlier && data.earliest && data.from ? <button className="text-button" onClick={() => onInterval(data.earliest!, new Date(new Date(data.from!).getTime()-1).toISOString())}>{data.earlier} earlier ↗</button> : null}{from ? stamp(from) : "All retained messages"}</span>
      <button ref={dateButton} className="timeline-date" aria-label="Choose dates and time basis" aria-expanded={custom} onClick={() => setCustom(!custom)}><Clock3 size={11}/>{scope.time_basis === "publication_time" ? "Published" : "Received"} · UTC{data?.undated ? ` · ${data.undated} undated` : ""}</button>
      <span>{to ? stamp(to) : ""}</span>
    </div>
    {custom && <div ref={options} className="timeline-popover" role="dialog" aria-label="Timeline dates">
      <header><strong>Time</strong><button aria-label="Close time options" onClick={() => { setCustom(false); dateButton.current?.focus(); }}><X size={15}/></button></header>
      <label>Message time<select aria-label="Message time basis" value={scope.time_basis} onChange={e => onBasis(e.target.value as Scope["time_basis"])}><option value="publication_time">Published by source</option><option value="acquisition_time">Received by Relic</option></select></label>
      <form onInput={e => (e.currentTarget.elements.namedItem("to") as HTMLInputElement).setCustomValidity("")} onSubmit={e => {
        e.preventDefault(); const fields = new FormData(e.currentTarget);
        const start = new Date(`${fields.get("from")}:00Z`), end = new Date(`${fields.get("to")}:00Z`);
        if (start > end) { (e.currentTarget.elements.namedItem("to") as HTMLInputElement).setCustomValidity("End must be after start."); e.currentTarget.reportValidity(); return; }
        onInterval(start.toISOString(), end.toISOString()); setCustom(false); dateButton.current?.focus();
      }}>
        <label>From · UTC<input name="from" aria-label="Timeline from UTC" type="datetime-local" required defaultValue={from?.slice(0, 16) ?? ""}/></label>
        <label>Through · UTC<input name="to" aria-label="Timeline through UTC" type="datetime-local" required defaultValue={to?.slice(0, 16) ?? ""} onChange={e => e.currentTarget.setCustomValidity("")}/></label>
        <button className="button" type="submit">Set interval</button>
      </form>
    </div>}
  </section>;
}
