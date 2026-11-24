import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { z } from "zod";
import { useState, type MouseEvent } from "react";
import { Anchor, Check, Clock3, MapPin, Search, X } from "lucide-react";
import { api } from "../api";
import { SHIP_COLOR, type ShipCategory } from "./live/symbols";
import "../vessel-finder.css";

/** Ships a message names: free detection, a paid extraction on request, free
 *  KeelWatch matching, and the analyst's decision. Suggestions only. */
const history = z.object({ closest_km: z.number().nullish(), place: z.string().nullish(), hours_from_message: z.number().nullish(), fixes: z.number().nullish() }).passthrough();
const candidate = z.object({
  key: z.string().nullish(), name: z.string().nullish(), type: z.string().nullish(), category: z.string().nullish(), flag: z.string().nullish(),
  mmsi: z.union([z.number(), z.string()]).nullish(), imo: z.union([z.number(), z.string()]).nullish(), length_m: z.number().nullish(),
  lon: z.number().nullish(), lat: z.number().nullish(), observed_at: z.string().nullish(), basis: z.string().nullish(), suggested: z.boolean().nullish(),
  history: history.nullish(), notes: z.array(z.object({ agrees: z.boolean(), text: z.string() })).nullish(),
}).passthrough();
const match = z.object({ candidates: z.array(candidate).default([]), similar_names: z.number().default(0), verdict: z.string().default(""), suggested: z.boolean().default(false), error: z.string().nullish() }).passthrough();
const mention = z.object({ name: z.string().nullish(), quote: z.string(), name_latin: z.string().nullish(), imo: z.string().nullish(), mmsi: z.string().nullish(), callsign: z.string().nullish(), hull_number: z.string().nullish(), flag: z.string().nullish(), vessel_type: z.string().nullish(), span: z.tuple([z.number(), z.number()]) });
/** When KeelWatch was not collecting in the 60 hours around the message. */
const coverage = z.object({ window_hours: z.number(), gap_hours: z.number(), gaps: z.array(z.object({ from: z.string(), to: z.string() })).default([]) }).nullish();
function CoverageNote({ c }: { c: z.infer<typeof coverage> }) {
  if (!c || c.gap_hours < 1) return null;
  return <p className="vessel-note vessel-gap">KeelWatch was not collecting for {Math.round(c.gap_hours)} of the {Math.round(c.window_hours)} hours around this message. A ship with no recorded position near the place may still have been there.</p>;
}
const schema = z.object({
  detection: z.object({ likely: z.boolean(), words: z.array(z.string()), identifiers: z.array(z.object({ kind: z.string(), value: z.string() })) }),
  lookup: z.object({ id: z.string(), status: z.string(), created_at: z.string(), cost_usd: z.number().nullish(), mentions: z.array(mention).default([]), error: z.string().nullish() }).nullish(),
  decisions: z.array(z.object({ mention: z.number(), vessel_key: z.string(), decision: z.string() })).default([]),
  reading_allowed: z.boolean(),
  matches: z.array(match).nullish(),
  identifier_matches: z.array(z.object({ identifier: z.object({ kind: z.string(), value: z.string() }), match: match.nullish() })).nullish(),
  coverage,
  // Paid extraction needs a passed evaluation of the current model profile;
  // without one (or from a server that does not say), Find ships stays off.
  evaluated: z.boolean().default(false),
}).passthrough();
const NEEDS_EVALUATION = "The ship finder needs a passed evaluation for its current model profile. Until then it sends no request.";
const nearby = z.object({
  coverage,
  from: z.string().nullish(), to: z.string().nullish(), note: z.string().nullish(),
  places: z.array(z.object({ label: z.string(), kind: z.string(), radius_km: z.number().nullish(), vessels: z.number().nullish(), truncated: z.boolean().nullish(), items: z.array(z.object({
    key: z.string().nullish(), name: z.string().nullish(), type: z.string().nullish(), category: z.string().nullish(), flag: z.string().nullish(), mmsi: z.union([z.number(), z.string()]).nullish(), imo: z.union([z.number(), z.string()]).nullish(),
    lon: z.number().nullish(), lat: z.number().nullish(), closest_km: z.number().nullish(), fixes: z.number().nullish(), first_hours: z.number().nullish(), last_hours: z.number().nullish(), observed_at: z.string().nullish(),
  }).passthrough()) })).default([]),
});
type Candidate = z.infer<typeof candidate>;
type Match = z.infer<typeof match>;
type Data = z.infer<typeof schema>;
type Decision = { mention: number; vessel_key: string; decision: "confirmed" | "rejected" | null };
/** The server's rule, applied to the cached reader: one confirmed vessel per
 *  mention (confirming another replaces it); null clears this vessel's decision. */
function applyDecision(list: Data["decisions"], d: Decision): Data["decisions"] {
  const rest = list.filter(x => x.mention !== d.mention || (x.vessel_key !== d.vessel_key && !(d.decision === "confirmed" && x.decision === "confirmed")));
  return d.decision ? [...rest, { mention: d.mention, vessel_key: d.vessel_key, decision: d.decision }] : rest;
}
/** Enough digits that a request costing a hundredth of a cent never reads as $0.000. */
const usd = (n: number) => n <= 0 ? "$0.00" : n < 0.0001 ? "<$0.0001" : `$${n.toFixed(n < 0.01 ? 4 : n < 0.1 ? 3 : 2)}`;
const message = (error: unknown, fallback: string) => error instanceof Error ? error.message : fallback;

const hours = (h: number) => Math.abs(h) < 1 ? "within the hour" : `${Math.round(Math.abs(h))} h ${h < 0 ? "before" : "after"} the message`;
function historyText(c: Candidate) {
  const h = c.history;
  if (!h) return null;
  if (h.closest_km == null) return "No recorded positions around the message time";
  return `${h.closest_km < 1 ? "Under 1 km" : `${Math.round(h.closest_km).toLocaleString("en-GB")} km`} from ${h.place}, ${hours(h.hours_from_message ?? 0)}`;
}
/** Ask the map to show every ship KeelWatch recorded at the message's time. */
function shipsAt(at: string | null, place: { lon?: number | null; lat?: number | null } | null) {
  if (!at) return;
  window.dispatchEvent(new CustomEvent("relic:ships-at", { detail: { at, lon: place?.lon, lat: place?.lat } }));
}
const span = (a: number | null | undefined, b: number | null | undefined) => {
  if (a == null || b == null) return null;
  if (a <= 0 && b >= 0) return "there at the message time";
  const h = Math.abs(a > 0 ? a : b);
  return `${h < 1 ? "within the hour" : `${Math.round(h)} h`} ${a > 0 ? "after" : "before"} the message`;
};

/** Ships KeelWatch recorded near the message's places around its time: for a
 *  message that names no ship, or to see what else was there. Free. */
function NearbyShips({ id, revision, publishedAt, gapShown, onShow }: { id: string; revision: string; publishedAt: string | null; gapShown: boolean; onShow: (c: Candidate) => void }) {
  const [open, setOpen] = useState(false);
  // Places whose longer list is open, each on its own.
  const [more, setMore] = useState<ReadonlySet<string>>(() => new Set());
  const data = useQuery({ queryKey: ["message-vessels-nearby", id, revision], enabled: open, staleTime: 300_000, retry: false,
    queryFn: ({ signal }) => api(`/evidence/vessels/nearby?id=${encodeURIComponent(id)}&revision=${encodeURIComponent(revision)}`, nearby, { signal }) });
  if (!open) return <button className="vessel-link" onClick={() => setOpen(true)}><MapPin size={12}/>Which ships were near the reported places?</button>;
  const d = data.data;
  const toggle = (place: string) => setMore(old => { const next = new Set(old); if (!next.delete(place)) next.add(place); return next; });
  return <div className="vessel-nearby">
    {data.isLoading && <p className="vessel-note" role="status">Looking through KeelWatch history…</p>}
    {data.error && <p className="vessel-note vessel-error" role="alert">{message(data.error, "KeelWatch history unavailable")}</p>}
    {d?.note && <p className="vessel-note">{d.note}</p>}
    {d && !gapShown && <CoverageNote c={d.coverage}/>}
    {d?.places.map((place, index) => {
      const placeKey = `${index}:${place.label}`;
      const expanded = more.has(placeKey);
      const shown = place.items.slice(0, expanded ? 40 : 6);
      const unlisted = place.items.length - shown.length;
      return <div key={placeKey} className="vessel-mention">
        <span className="vessel-quote">{place.kind === "point" ? `Within ${place.radius_km} km of ${place.label}` : `Inside ${place.label}`}</span>
        <p className="vessel-note">{place.vessels ? `${place.vessels.toLocaleString("en-GB")} ${place.vessels === 1 ? "vessel" : "vessels"} recorded from 48 h before to 12 h after the message${place.kind === "point" ? ", nearest first" : ""}.` : "KeelWatch recorded no vessel there in that window. That does not mean none was there."}{place.truncated ? " KeelWatch returned only the first ones." : ""}</p>
        {shown.length > 0 && <ul className="vessel-candidates">{shown.map(v => <li key={v.key ?? `${v.mmsi}`} className="vessel-candidate">
          <i style={{ background: SHIP_COLOR[(v.category as ShipCategory) ?? "other"] ?? SHIP_COLOR.other }}/>
          <div>
            <strong>{v.name ?? "Unnamed vessel"}</strong>
            <small>{[v.type, v.flag, v.closest_km != null ? `${v.closest_km < 1 ? "under 1" : Math.round(v.closest_km)} km away at closest` : null].filter(Boolean).join(" · ")}</small>
            {span(v.first_hours, v.last_hours) && <small>{span(v.first_hours, v.last_hours)}{v.fixes ? ` · ${v.fixes} ${v.fixes === 1 ? "report" : "reports"}` : ""}</small>}
            <div className="vessel-actions"><button onClick={() => onShow({ key: v.key, name: v.name, category: v.category, mmsi: v.mmsi, imo: v.imo, lon: v.lon, lat: v.lat })} disabled={!v.key}><MapPin size={12}/>Show on map</button></div>
          </div>
        </li>)}</ul>}
        {expanded && unlisted > 0 && <p className="vessel-note">{unlisted.toLocaleString("en-GB")} more not shown.</p>}
        {place.items.length > 6 && <button className="vessel-link" aria-expanded={expanded} onClick={() => toggle(placeKey)}>{expanded ? "Show fewer" : `Show ${Math.min(40, place.items.length) - 6} more`}</button>}
      </div>;
    })}
    {publishedAt && d && d.places.length > 0 && <button className="vessel-find" onClick={() => shipsAt(publishedAt, null)}><Clock3 size={13}/>Every ship at this moment on the map</button>}
  </div>;
}

/** Ask the live layers to show this vessel with its track around the message. */
function showOnMap(c: Candidate, at: string | null) {
  const t = at ? Date.parse(at) : Date.now();
  window.dispatchEvent(new CustomEvent("relic:show-vessel", { detail: { key: c.key, name: c.name, category: c.category, mmsi: c.mmsi, imo: c.imo, lon: c.lon, lat: c.lat,
    from: new Date(t - 48 * 3_600_000).toISOString(), to: new Date(Math.min(Date.now(), t + 12 * 3_600_000)).toISOString() } }));
}

export default function VesselFinder({ id, revision, publishedAt, onQuote }: { id: string; revision: string; publishedAt: string | null; onQuote: (start: number, end: number) => void }) {
  const client = useQueryClient();
  const key = ["message-vessels", id, revision];
  const data = useQuery({ queryKey: key, queryFn: ({ signal }) => api(`/evidence/vessels?id=${encodeURIComponent(id)}&revision=${encodeURIComponent(revision)}`, schema, { signal }), staleTime: 60_000, retry: false,
    // A batch run may hold this message's one attempt; follow it until it ends.
    refetchInterval: q => q.state.data?.lookup?.status === "reserved" ? 3000 : false });
  const find = useMutation({
    mutationFn: () => api("/evidence/vessels", schema, { method: "POST", body: { id, revision } }),
    onSuccess: value => { client.setQueryData(key, value); void client.invalidateQueries({ queryKey: ["vessel-marks"] }); },
    onError: () => { void client.invalidateQueries({ queryKey: key }); },
  });
  // Decisions update the cached reader in place: refetching it would re-run
  // every KeelWatch match. A failed save restores the previous decisions.
  const decide = useMutation({
    mutationFn: (d: Decision) => api("/evidence/vessels/decision", z.object({ saved: z.boolean() }), { method: "PUT", body: { lookup: data.data?.lookup?.id, ...d } }),
    onMutate: async (d: Decision) => {
      await client.cancelQueries({ queryKey: key });
      const before = client.getQueryData<Data>(key);
      if (before) client.setQueryData<Data>(key, { ...before, decisions: applyDecision(before.decisions, d) });
      return { before };
    },
    onError: (_error, _d, context) => { if (context?.before) client.setQueryData(key, context.before); },
    onSettled: () => { for (const k of [["vessel-marks"], ["live-ship-messages"]]) void client.invalidateQueries({ queryKey: k }); },
  });
  const d = data.data;
  if (!d) {
    if (data.isLoading) return <section className="vessel-finder" aria-label="Ships named in this message" aria-busy="true"><p className="vessel-note" role="status">Checking for ships…</p></section>;
    if (data.error) return <section className="vessel-finder" aria-label="Ships named in this message">
      <header><h3><Anchor size={13}/>Ships</h3></header>
      <p className="vessel-note vessel-error" role="alert">Ships could not be checked: {message(data.error, "the request failed")}</p>
      <button className="vessel-link" disabled={data.isFetching} onClick={() => void data.refetch()}>{data.isFetching ? "Checking…" : "Try again"}</button>
    </section>;
    return null;
  }
  const lookup = d.lookup;
  const direct = (d.identifier_matches ?? []).filter(m => m.match?.candidates.length);
  if (!d.detection.likely && !lookup && !direct.length) return null;
  // The collection-gap note is shown once: above the matches when there are any.
  const gapAbove = lookup?.status === "completed" || direct.length > 0;
  const decided = (mention: number, vessel: string) => d.decisions.find(x => x.mention === mention && x.vessel_key === vessel)?.decision;
  const same = (v: Decision | undefined, mention: number | null, vessel: string) => !!v && v.mention === mention && v.vessel_key === vessel;

  const row = (c: Candidate, mentionIndex: number | null) => {
    const vessel = c.key ?? "";
    const state = mentionIndex == null ? undefined : decided(mentionIndex, vessel);
    const saving = decide.isPending && same(decide.variables, mentionIndex, vessel);
    const failed = decide.isError && same(decide.variables, mentionIndex, vessel);
    const facts = [c.type, c.flag, c.imo ? `IMO ${c.imo}` : c.mmsi ? `MMSI ${c.mmsi}` : null, c.length_m ? `${c.length_m} m` : null].filter(Boolean).join(" · ");
    const pick = (event: MouseEvent<HTMLButtonElement>, decision: Decision["decision"]) => {
      if (mentionIndex == null) return;
      // The clicked button leaves with the new state: keep focus on its candidate.
      event.currentTarget.closest(".vessel-candidate")?.querySelector<HTMLElement>(".vessel-name")?.focus();
      decide.mutate({ mention: mentionIndex, vessel_key: vessel, decision });
    };
    return <li key={vessel} className={`vessel-candidate ${state ?? ""}`}>
      <i style={{ background: SHIP_COLOR[(c.category as ShipCategory) ?? "other"] ?? SHIP_COLOR.other }}/>
      <div>
        <strong className="vessel-name" tabIndex={-1}>{c.name ?? "Unnamed vessel"}
          {c.suggested && !state && <span className="vessel-tag" title="The ship finder's best match from KeelWatch. It is a suggestion, not a decision.">Suggested</span>}
          {state === "confirmed" && <span className="vessel-state"><Check size={11}/>Confirmed</span>}{state === "rejected" && <span className="vessel-state">Not this ship</span>}</strong>
        {c.suggested && !state && mentionIndex != null && <small className="vessel-suggestion">A suggestion until you choose This ship.</small>}
        {facts && <small>{facts}</small>}
        {c.basis && c.basis !== "same name" && <small className="vessel-basis">{c.basis.charAt(0).toUpperCase() + c.basis.slice(1)}</small>}
        {historyText(c) && <small className={c.history?.closest_km != null && c.history.closest_km <= 200 ? "vessel-near" : ""}>{historyText(c)}</small>}
        {c.observed_at && <small>Last reported {new Date(c.observed_at).toISOString().slice(0, 16).replace("T", " ")} UTC</small>}
        {c.notes?.map(n => <small key={n.text} className={n.agrees ? "" : "vessel-disagrees"}>{n.agrees ? "Matches: " : "Differs: "}{n.text}</small>)}
        <div className="vessel-actions">
          <button onClick={() => showOnMap(c, publishedAt)} disabled={!c.key}><MapPin size={12}/>Show on map</button>
          {mentionIndex != null && state !== "confirmed" && <button disabled={decide.isPending || !vessel} onClick={e => pick(e, "confirmed")}><Check size={12}/>This ship</button>}
          {mentionIndex != null && !state && <button disabled={decide.isPending || !vessel} onClick={e => pick(e, "rejected")}><X size={12}/>Not this ship</button>}
          {mentionIndex != null && state && <button disabled={decide.isPending} onClick={e => pick(e, null)}>Undo</button>}
        </div>
        {saving && <small role="status">Saving…</small>}
        {failed && <small className="vessel-disagrees" role="alert">Not saved: {message(decide.error, "the decision could not be saved")}</small>}
      </div>
    </li>;
  };
  const candidates = (m: Match, mentionIndex: number | null, label: string) => {
    if (m.error) return <p className="vessel-note vessel-error">{m.error}</p>;
    const [first, ...rest] = m.candidates;
    return <>
      <p className="vessel-note">{m.verdict}{m.similar_names ? ` ${m.similar_names} other vessel${m.similar_names === 1 ? " has" : "s have"} a similar name.` : ""}</p>
      {first && <ul className="vessel-candidates">{row(first, mentionIndex)}</ul>}
      {rest.length > 0 && <details className="vessel-more"><summary>{rest.length} more vessel{rest.length === 1 ? "" : "s"} named {label}</summary><ul className="vessel-candidates">{rest.map(c => row(c, mentionIndex))}</ul></details>}
    </>;
  };
  const findButton = (text: string) => <>
    <button className="vessel-find" disabled={!d.reading_allowed || !d.evaluated || find.isPending} title={d.evaluated ? undefined : NEEDS_EVALUATION} onClick={() => find.mutate()}><Search size={13}/>{find.isPending ? "Finding ships…" : text}</button>
    {!d.evaluated && <p className="vessel-note">{NEEDS_EVALUATION}</p>}
  </>;

  return <section className="vessel-finder" aria-label="Ships named in this message">
    <header><h3><Anchor size={13}/>Ships</h3>{lookup?.status === "completed" && <small>Found with one model request · {lookup.cost_usd != null ? usd(lookup.cost_usd) : "cost unknown"}</small>}</header>
    {data.isRefetchError && <p className="vessel-note vessel-error" role="alert">Could not refresh: {message(data.error, "the request failed")}. Showing the last result.</p>}
    {gapAbove && <CoverageNote c={d.coverage}/>}
    {direct.map(m => <div className="vessel-mention" key={m.identifier.value}>
      <span className="vessel-quote">{m.identifier.kind.toUpperCase()} {m.identifier.value}</span>
      {m.match && candidates(m.match, null, `${m.identifier.kind.toUpperCase()} ${m.identifier.value}`)}
    </div>)}
    {!lookup && <div className="vessel-offer">
      <p className="vessel-note">This message may name ships{d.detection.words.length ? ` (${d.detection.words.slice(0, 3).map(w => w.trim()).join(", ")})` : ""}. Finding them sends its text to the model once, then looks each one up in KeelWatch.</p>
      {findButton("Find ships")}
      {!d.reading_allowed && <p className="vessel-note">Reading is off for this source, so its text is not sent to the model. Turn reading on in Sources to use this.</p>}
    </div>}
    {lookup?.status === "reserved" && <p className="vessel-note" role="status">Finding ships… The model request is in progress.</p>}
    {lookup && ["failed", "unknown"].includes(lookup.status) && <div className="vessel-offer">
      <p className="vessel-note vessel-error">{lookup.error ?? "The ship finder failed."}</p>
      <p className="vessel-note">{lookup.status === "unknown" ? "That request may have been charged; its cost is unknown. " : ""}Trying again sends one new model request.</p>
      {findButton("Try again")}
    </div>}
    {find.error && <p className="vessel-note vessel-error" role="alert">{message(find.error, "The ship finder failed.")}</p>}
    {lookup?.status === "completed" && !lookup.mentions.length && <p className="vessel-note">No named ships in this message.</p>}
    {lookup?.status === "completed" && lookup.mentions.map((m, i) => {
      const label = m.name ?? m.imo ?? m.mmsi ?? m.hull_number ?? m.callsign ?? "vessel";
      const found = d.matches?.[i];
      return <div className="vessel-mention" key={i}>
        <button className="vessel-quote" dir="auto" title="Show where the message says this" onClick={() => onQuote(m.span[0], m.span[1])}>{label}</button>
        {m.name_latin && m.name_latin !== m.name && <small className="vessel-latin">searched as {m.name_latin}</small>}
        {found ? candidates(found, i, m.name_latin ?? label) : <p className="vessel-note">Looking up KeelWatch…</p>}
      </div>;
    })}
    <NearbyShips id={id} revision={revision} publishedAt={publishedAt} gapShown={gapAbove} onShow={c => showOnMap(c, publishedAt)}/>
  </section>;
}
