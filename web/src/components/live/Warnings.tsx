import { useState, type CSSProperties } from "react";
import type { Feature, Point } from "geojson";
import { text, utc, type Props } from "./data";

/** MSCIO's own type words, hottest first: the map and the list share them. */
export const WARNING_COLOR: Record<string, string> = { Attack: "#ee6b6b", Hijack: "#ee6b6b", Kidnap: "#ee6b6b", Boarding: "#e3a857", "Attempted Boarding": "#e3a857", Piracy: "#e3a857", "Armed Robbery": "#e3a857", "Suspicious Activity": "#e8d27a" };
export const WARNING_OTHER = "#9fb3c2";
export const warningColorOf = (type: unknown) => WARNING_COLOR[String(type ?? "")] ?? WARNING_OTHER;
/** Attacks, hijacks and kidnaps draw larger than approaches and other crimes. */
export const HOT_WARNINGS = ["Attack", "Hijack", "Kidnap"];

/** Registration windows, in hours; "all" is every retained warning. */
export type WarningWindow = "6" | "24" | "72" | "168" | "720" | "2160" | "all";
export const WARNING_WINDOWS: { id: WarningWindow; short: string; long: string }[] = [
  { id: "6", short: "6 h", long: "6 hours" }, { id: "24", short: "24 h", long: "24 hours" }, { id: "72", short: "3 d", long: "3 days" },
  { id: "168", short: "7 d", long: "7 days" }, { id: "720", short: "30 d", long: "30 days" }, { id: "2160", short: "90 d", long: "90 days" },
  { id: "all", short: "All", long: "all retained" },
];
export const windowLabel = (w: WarningWindow) => WARNING_WINDOWS.find(x => x.id === w)?.long ?? "all retained";
/** Older saved settings counted whole days. */
export const fromDays = (days: unknown): WarningWindow | null => ({ "7": "168", "30": "720", "90": "2160", all: "all" } as Record<string, WarningWindow>)[String(days)] ?? null;

/** How long ago, in the list's compact form: "40 min", "9 h", "3 d". */
const since = (iso: unknown, now: number) => {
  const t = Date.parse(String(iso));
  if (!Number.isFinite(t)) return "?";
  const m = Math.max(0, (now - t) / 60_000);
  return m < 90 ? `${Math.round(m)} min` : m < 2880 ? `${Math.round(m / 60)} h` : `${Math.round(m / 1440)} d`;
};
/** A list line from the warning's words: without the leading "ALERT nn/yy –
 *  TYPE" reference and MSCIO's opening formula ("MSCIO has received … an
 *  incident at approximately 08:39 UTC on 23 October 2026"), so the place and
 *  what happened come first. The card keeps the whole text. */
export const excerpt = (p: Props) => {
  // Some texts repeat the reference ("ALERT 117/26 ALERT 117/26 – ATTACK MSCIO …").
  const plain = String(p.text ?? "").replace(/\s+/g, " ").trim().replace(/^(?:ALERT\s+\d+\/\d+\s*)+(?:[–-]\s*[A-Z][A-Z ]+?\s+(?=MSCIO))?/i, "");
  const rest = plain.replace(/^MSCIO has received [^.]*?\bincident\b(?: at approximately [0-9:]+ UTC)?(?: on \d{1,2} \w+ \d{4})?[ ,]*/i, "");
  return rest && rest !== plain ? rest.charAt(0).toUpperCase() + rest.slice(1) : plain;
};

export function WarningWindowControl({ value, onChange, past }: { value: WarningWindow; onChange: (w: WarningWindow) => void; past: boolean }) {
  return <div className="live-window" role="group" aria-label={past ? "Warnings registered in the time before the chosen moment" : "Warnings registered within"}>
    <small>{past ? "Registered before then" : "Registered in the last"}</small>
    <div className="live-segment">
      {WARNING_WINDOWS.map(w => <button key={w.id} aria-pressed={value === w.id} aria-label={w.id === "all" ? "All retained warnings" : w.long} onClick={() => onChange(w.id)}>{w.short}</button>)}
    </div>
  </div>;
}

/** The warnings in the window, newest first; choosing one shows it on the map. */
export function LatestWarnings({ items, now, onPick }: { items: Feature[]; now: number; onPick: (f: Feature, keyboard: boolean) => void }) {
  const [all, setAll] = useState(false);
  if (!items.length) return null;
  const shown = all ? items.slice(0, 50) : items.slice(0, 4);
  return <div className="live-warning-list">
    <ul role="list" aria-label="Warnings, newest first">
      {shown.map(f => {
        const p = (f.properties ?? {}) as Props;
        const unlisted = p.listed === false;
        const color = warningColorOf(p.type);
        const [lon, lat] = (f.geometry as Point).coordinates;
        return <li key={String(p.id)}><button onClick={e => onPick(f, e.detail === 0)} title={`Registered ${utc(Date.parse(String(p.registered_at)))} · ${lat.toFixed(2)}, ${lon.toFixed(2)}`}>
          <i className={unlisted ? "unlisted" : undefined} style={{ "--warn": color } as CSSProperties} aria-hidden/>
          <span><strong>{text(p.type) ?? "Warning"}{text(p.reference) ? <small> {String(p.reference)}</small> : null}</strong><em dir="auto">{unlisted ? "No longer listed · " : ""}{excerpt(p)}</em></span>
          <time dateTime={String(p.registered_at)}>{since(p.registered_at, now)}</time>
        </button></li>;
      })}
    </ul>
    {items.length > 4 && <button className="live-link" onClick={() => setAll(!all)}>{all ? "Show fewer" : `Show all ${Math.min(items.length, 50)}${items.length > 50 ? " newest" : ""}`}</button>}
  </div>;
}
