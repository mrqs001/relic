import { CompactTime, MessageTime } from "./MessageTime";
import { useEffect, useLayoutEffect, useMemo, useRef, type CSSProperties } from "react";
import { defaultRangeExtractor, useVirtualizer } from "@tanstack/react-virtual";
import { useQueries } from "@tanstack/react-query";
import { z } from "zod";
import {
  Anchor,
  ArrowDown,
  Link2,
  MapPinOff,
  X,
} from "lucide-react";
import { api, eventTypes } from "../api";
import type { EvidencePreview, QueryResult, Scope } from "../api";
import { Busy, Empty, ErrorMessage } from "./Common";
import { claimStatusLabel, readingPreview, cleanReadingText, feedPlace } from "./reading-display";
import { eventColors, eventLabels, eventGroups, eventGroup, possibleTitle, primaryEvent } from "./EventLabel";
import "../reading.css";
import "../feed.css";
import { feedRows } from "./feed-albums";
import { SHIP_COLOR, type ShipCategory } from "./live/symbols";

const marksSchema = z.object({ marks: z.record(z.string(), z.array(z.object({ name: z.string().nullish(), quoted: z.string().nullish(), category: z.string().nullish(), state: z.string(), vessel_key: z.string() }))) });
type Mark = z.infer<typeof marksSchema>["marks"][string][number];
type MarkChunk = { key: string; items: { id: string; revision: string }[] };
const FNV = 2166136261;
const fnv = (text: string, h = FNV) => { for (let i = 0; i < text.length; i++) h = Math.imul(h ^ text.charCodeAt(i), 16777619); return h >>> 0; };
/** Loaded messages in chunks whose boundaries depend on the messages, not on
 *  their positions: a chunk ends after a message whose key hashes to 0 mod
 *  128 (or at 400, under the server's 500). New messages at the top or an
 *  older page at the bottom change only the first or last chunk, so the
 *  others keep their cached marks. A chunk's query key is compact. */
function markChunks(items: EvidencePreview[]): MarkChunk[] {
  const out: MarkChunk[] = [];
  let chunk: MarkChunk["items"] = [], h = FNV;
  for (const item of items) {
    const key = `${item.id}|${item.revision}`;
    chunk.push({ id: item.id, revision: item.revision });
    h = fnv(`${key},`, h);
    if (fnv(key) % 128 === 0 || chunk.length === 400) { out.push({ key: `${chunk.length}:${h.toString(36)}`, items: chunk }); chunk = []; h = FNV; }
  }
  if (chunk.length) out.push({ key: `${chunk.length}:${h.toString(36)}`, items: chunk });
  return out;
}
/** Ships the ship finder links to loaded messages: confirmed, or suggested
 *  and not rejected. While a chunk loads, its rows keep the marks they had;
 *  a failed chunk is reported, never shown as "no ships". */
function useVesselMarks(items: EvidencePreview[]): { marks: Map<string, Mark[]>; error: unknown; partial: boolean } {
  const chunks = useMemo(() => markChunks(items), [items]);
  const results = useQueries({ queries: chunks.map((chunk, index) => ({
    queryKey: ["vessel-marks", chunk.key], staleTime: 30_000, retry: false,
    // The newest messages are the ones the ship finder is working through.
    refetchInterval: index < 2 ? 60_000 : 300_000,
    queryFn: ({ signal }: { signal: AbortSignal }) => api("/evidence/vessels/marks", marksSchema, { method: "POST", body: { items: chunk.items }, signal }),
  })) });
  const last = useRef(new Map<string, Mark[]>());
  const updated = results.map(r => r.dataUpdatedAt).join(",");
  const marks = useMemo(() => {
    const next = new Map<string, Mark[]>();
    chunks.forEach((chunk, index) => {
      const data = results[index]?.data?.marks;
      for (const { id, revision } of chunk.items) {
        const key = `${id}|${revision}`;
        const found = data ? data[key] : last.current.get(key);
        if (found?.length) next.set(key, found);
      }
    });
    last.current = next;
    return next;
  }, [chunks, updated]); // `updated` stands for the results' data, which change identity every render
  const failed = results.filter(r => r.error && !r.data);
  return { marks, error: failed[0]?.error ?? null, partial: failed.length < chunks.length };
}
export default function EvidenceList({
  items: retainedItems,
  result,
  selected,
  onSelect,
  updating,
  error,
  hasMore,
  loadMore,
  fetchingMore,
  contentView,
  onContentView,
  eventType,
  onEventType,
  relatedKey,
  onRelated,
}: {
  items: EvidencePreview[];
  result?: QueryResult;
  selected: { id: string | null; revision: string | null };
  onSelect: (id: string, revision: string) => void;
  updating: boolean;
  error: unknown;
  hasMore: boolean;
  loadMore: () => void;
  fetchingMore: boolean;
  contentView: Scope["content_view"];
  onContentView: (view: Scope["content_view"], clearType?: boolean) => void;
  eventType: Scope["event_type"];
  onEventType: (type: Scope["event_type"]) => void;
  relatedKey: string | null;
  onRelated: (key: string | null) => void;
}) {
  const albums = useMemo(()=>feedRows(retainedItems),[retainedItems]);
  const items = useMemo(()=>albums.map(album=>album.message),[albums]);
  const { marks: vesselMarks, error: marksError, partial: marksPartial } = useVesselMarks(retainedItems);
  const host = useRef<HTMLDivElement>(null);
  const rowButtons = useRef(new Map<string, HTMLButtonElement>());
  const pendingKeyboardFocus = useRef<string | null>(null);
  const isChosen = (i: EvidencePreview) => i.id===selected.id && i.revision===selected.revision;
  const selectedIndex = albums.findIndex(album=>album.members.some(isChosen) || album.reports.some(r=>r.members.some(isChosen)));
  const rows = useVirtualizer({
    rangeExtractor: (range) =>
      [
        ...new Set([
          ...defaultRangeExtractor(range),
          ...(selectedIndex >= 0 ? [selectedIndex] : []),
        ]),
      ].sort((a, b) => a - b),
    count: items.length,
    getScrollElement: () => host.current,
    estimateSize: () => 172,
    overscan: 5,
    getItemKey: (i) => `${items[i].id}@${items[i].revision}`,
  });
  useEffect(() => {
    if (result && !updating) performance.mark("relic:query-visible");
  }, [result?.scope_revision, result?.generation, updating]);
  // Move focus only for an explicit arrow-key selection, after the new row
  // has mounted. Mouse selections and query refreshes never request focus.
  useLayoutEffect(() => {
    const pending = pendingKeyboardFocus.current;
    if (!pending || pending !== `${selected.id}@${selected.revision}`) return;
    const button = rowButtons.current.get(pending);
    if (!button) return;
    button.focus({ preventScroll: true });
    pendingKeyboardFocus.current = null;
  });
  const navigate = (index: number) => {
    const item = items[Math.max(0, Math.min(items.length - 1, index))];
    if (!item) return;
    pendingKeyboardFocus.current = `${item.id}@${item.revision}`;
    onSelect(item.id, item.revision);
    rows.scrollToIndex(Math.max(0, Math.min(items.length - 1, index)), {
      align: "auto",
    });
  };
  return (
    <section
      className={`evidence-list ${updating ? "is-updating" : ""}`}
      aria-label="Evidence results"
      data-testid="query-results"
      data-generation={result?.generation}
      data-publication={result?.publication_id}
      data-scope={result?.scope_revision}
      aria-busy={updating}
    >
      <div className="list-heading">
        <div>
          <span className="eyebrow">Retained reporting</span>
          <h2>
            Evidence <span>{result?.total ?? "—"}</span>
          </h2>
        </div>
        <span className="sort-label">
          Newest first <ArrowDown size={12} />
        </span>
      </div>
      <div className="feed-filters">
        <div className="reading-view-control" role="group" aria-label="Feed content"><button aria-pressed={contentView === "events"} onClick={() => onContentView("events")} title="Processed messages reporting an occurrence. These are source claims, not verified or grouped incidents.">Event reports</button><button aria-pressed={contentView === "all"} onClick={() => onContentView("all")} title="All retained messages within these filters, including unread posts and commentary.">All messages</button></div>
        <select aria-label="Event type" value={eventType || ""} onChange={e => onEventType(e.target.value as Scope["event_type"] || null)}><option value="">All types</option>{eventGroups.map(group => <optgroup key={group} label={group}>{eventTypes.filter(type => eventGroup[type] === group).map(type => <option key={type} value={type}>{eventLabels[type]}</option>)}</optgroup>)}</select>
      </div>
      {contentView === "signals" && <button className="legacy-feed-mode" onClick={() => onContentView("all")}>Legacy Signals filter · Show all messages</button>}
      {relatedKey && <div className="related-feed-context"><div><strong>Related reporting</strong><small>Same city, topic and day. Origins may be shared.</small></div><button className="icon-button" aria-label="Clear related reporting" onClick={() => onRelated(null)}><X size={14}/></button></div>}
      <ErrorMessage error={error} />
      {updating && (
        <div className="list-updating" role="status">
          Updating · previous scope shown
        </div>
      )}
      <div
        className="virtual-list"
        ref={host}
        tabIndex={0}
        aria-label="Evidence list. Use up and down arrows to select evidence."
        onKeyDown={(event) => {
          if ((event.target as HTMLElement).closest(".album-members")) return;
          if (
            event.target !== event.currentTarget &&
            (event.target as HTMLElement).tagName !== "BUTTON"
          )
            return;
          const current = selectedIndex;
          if (event.key === "ArrowDown" || event.key === "ArrowUp") {
            event.preventDefault();
            navigate(current + (event.key === "ArrowDown" ? 1 : -1));
          }
        }}
      >
        {!result && !error && <Busy />}
        {result && items.length === 0 && (
          <Empty title={contentView === "events" ? "No matching events yet" : "No matching messages"} icon={<MapPinOff size={28} />}>
            {contentView === "events" ? <>Messages awaiting classification remain in All messages.<button className="text-button" onClick={() => { onContentView("all", true); }}>Show all messages</button></> : "Try another type or clear the map and time filters."}
          </Empty>
        )}
        <div style={{ height: rows.getTotalSize(), position: "relative" }}>
          {rows.getVirtualItems().map((row) => {
            const item = items[row.index],
              isSelected =
                row.index === selectedIndex;
            const interpretation = item.interpretation;
            const ships = vesselMarks.get(`${item.id}|${item.revision}`) ?? [];
            // Where and what first: the place, the event and its map colour.
            const { primary: event, kinds } = primaryEvent(interpretation, eventType);
            const where = feedPlace(item);
            const place = where?.place ?? "";
            const source = item.source_label.replace(/^https?:\/\/t\.me\//,"@").replace("Fictional ", "");
            const reports = item.event_group?.reports ?? (item.event_links ? item.event_links + 1 : 0);
            // A reported place leads; without one, the event itself does.
            // Jev's suggestion, only where the main reading found no event at all.
            const possible = event ? undefined : item.possible_event;
            const typeText = event ? `${eventLabels[event.type]}${event.state !== "reported" ? ` · ${claimStatusLabel[event.state].toLowerCase()}` : ""}${kinds.length > 1 ? ` +${kinds.length - 1}` : ""}` : "";
            return (
              <div
                key={row.key}
                className="feed-item"
                data-index={row.index}
                ref={rows.measureElement}
                style={{
                  position: "absolute",
                  top: 0,
                  left: 0,
                  width: "100%",
                  transform: `translateY(${row.start}px)`,
                }}
              >
                <button
                  className={`evidence-row feed-row ${isSelected ? "selected" : ""} ${interpretation ? "processed" : ""}`}
                  style={{ "--event": event ? eventColors[event.type] : possible ? eventColors[possible.type] : undefined } as CSSProperties}
                  aria-pressed={isSelected}
                  ref={(button) => {
                    const key = `${item.id}@${item.revision}`;
                    if (button) rowButtons.current.set(key, button);
                    else rowButtons.current.delete(key);
                  }}
                  onClick={() => {
                    pendingKeyboardFocus.current = null;
                    onSelect(item.id, item.revision);
                  }}
                  data-testid={`evidence-${item.id}`}
                >
                  <CompactTime {...item}/>
                  <i className={`feed-mark ${event ? "" : possible ? "possible" : "none"}`} aria-hidden/>
                  <span className="feed-head">
                    <span className="feed-place" dir="auto" title={place && item.map_locations.some(site => site.approximate) ? "Includes approximate reference locations" : undefined}>{where ? <>{where.place}{where.country && <span className="feed-country">{where.country}</span>}{where.more > 0 && <span className="feed-country">+{where.more}</span>}</> : event ? typeText : possible ? eventLabels[possible.type] : source}</span>
                    {reports > 1 && <span className="feed-reports" title="Reports grouped as one event: linked by review, or the same kind of event at the same place within a few hours. Not independent confirmation.">{reports} reports</span>}
                    {event && <span className="feed-type" title={`Source claims, inferred from the text: ${kinds.map(e => `${eventLabels[e.type]} (${claimStatusLabel[e.state].toLowerCase()})`).join(", ")}`}>{place ? typeText : "no place"}</span>}
                    {possible && <span className="feed-type possible" title={possibleTitle(possible)}>{place ? `${eventLabels[possible.type]} · possible` : "possible"}</span>}
                    {!event && !possible && interpretation && ["denied","planned","historical"].includes(interpretation.claim_status) && <span className="feed-type">{claimStatusLabel[interpretation.claim_status].toLowerCase()}</span>}
                  </span>
                  {/* The source's own words (translated), as in Recall; the model headline
                      ("A report says…") stays in the reader. */}
                  <span className="feed-text" dir="auto">{readingPreview(item)}</span>
                  <span className="feed-meta">
                    {(place || event || possible) && <span className="feed-source" dir="auto">{source}</span>}
                    {ships.slice(0, 3).map(ship => <span key={`${ship.vessel_key}`} className={`vessel-chip ${ship.state}`} style={{ "--ship": SHIP_COLOR[(ship.category as ShipCategory) ?? "other"] ?? SHIP_COLOR.other } as CSSProperties}
                      title={ship.state === "confirmed" ? "Ship confirmed in the ship finder" : "Ship suggested by the ship finder; not confirmed"}><Anchor size={10} aria-hidden/>
                      <span className="visually-hidden">{ship.state === "confirmed" ? "Confirmed ship: " : "Suggested ship: "}</span>
                      <span className="vessel-chip-name">{ship.name ?? ship.quoted ?? "Vessel"}</span>{ship.state !== "confirmed" && <span className="vessel-chip-cue" aria-hidden>?</span>}</span>)}
                    {ships.length > 3 && <span className="vessel-chip more" title={`${ships.length - 3} more ships linked by the ship finder`}>+{ships.length - 3}<span className="visually-hidden"> more ships</span></span>}
                  </span>
                </button>
                {albums[row.index].reports.length>0 && <details className="event-members" open={albums[row.index].reports.some(r=>r.members.some(isChosen)) || undefined}>
                  <summary title="Reports grouped as one event: linked by review, or the same kind of event at the same place within a few hours. Not independent confirmation."><Link2 size={11}/>Same event · {item.event_group?.reports ?? albums[row.index].reports.length+1} reports</summary>
                  {albums[row.index].reports.map(report=>{const m=report.message;return <button key={`${m.id}@${m.revision}`} aria-current={report.members.some(isChosen) || undefined} onClick={()=>onSelect(m.id,m.revision)}>
                    <span className="event-member-meta"><span dir="auto">{m.source_label.replace(/^https?:\/\/t\.me\//,"@").replace("Fictional ", "")}</span><MessageTime {...m}/></span>
                    <span className="event-member-text" dir="auto">{m.interpretation?.headline ? cleanReadingText(m.interpretation.headline) : readingPreview(m)}</span>
                  </button>;})}
                  {(item.event_group?.reports ?? 0) > albums[row.index].reports.length+1 && <small>{(item.event_group!.reports)-albums[row.index].reports.length-1} more in older messages. The message shows them all.</small>}
                </details>}
                {albums[row.index].members.length>1 && <details className="album-members">
                  <summary>Album · {albums[row.index].members.length} loaded messages</summary>
                  {albums[row.index].members.map((member,index)=><button className="text-button" key={`${member.id}@${member.revision}`} onClick={()=>onSelect(member.id,member.revision)}><MessageTime {...member}/><span>{index+1}. {member.text ? "Caption and media" : "Media"} ↗</span></button>)}
                </details>}
              </div>
            );
          })}
        </div>
        {hasMore && (
          <button
            className="load-more"
            onClick={loadMore}
            disabled={fetchingMore}
          >
            {fetchingMore ? "Loading…" : "Load more"}
          </button>
        )}
      </div>
      <footer className="list-footer">
        <span title={`${retainedItems.length} messages loaded as ${items.length} rows`}>
          {items.length.toLocaleString("en-GB")}{result ? ` of ${result.total.toLocaleString("en-GB")}` : ""}
        </span>
        {!!marksError && <span className="vessel-marks-error" role="status" title={marksError instanceof Error ? marksError.message : String(marksError)}>{marksPartial ? "Some ship marks unavailable" : "Ship marks unavailable"}</span>}
        <span>{result?.unread ? `${result.unread} unread` : `${result?.unlocated ?? 0} unlocated`}</span>
      </footer>
    </section>
  );
}
