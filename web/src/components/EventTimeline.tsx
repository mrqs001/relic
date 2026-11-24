import { useState, type CSSProperties } from "react";
import { useQuery } from "@tanstack/react-query";
import { z } from "zod";
import { Crosshair } from "lucide-react";
import { api, evidenceSchema, type Evidence, type Scope } from "../api";
import { readingPreview } from "./reading-display";
import { MessageTime } from "./MessageTime";
import { ErrorMessage } from "./Common";
import "../event-timeline.css";

const relation = z.enum(["same_occurrence", "update", "denial", "correction"]);
const groupSchema = z.object({
  claim: z.object({ id: z.string(), text: z.string(), type: z.string() }),
  truncated: z.boolean(),
  members: z.array(z.object({ evidence: evidenceSchema, anchor: z.boolean(), claims: z.array(z.object({ id: z.string(), text: z.string(), state: z.string(), type: z.string() })) })),
  links: z.array(z.object({ a: z.string(), b: z.string(), relation, a_state: z.string(), b_state: z.string() })),
});
const responseSchema = z.object({ publication_id: z.string(), groups: z.array(groupSchema) });
type Group = z.infer<typeof groupSchema>;
type Member = Group["members"][number];
const SHOWN = 5;

/** How a member relates to the report it links to (the anchor when directly linked). */
function tag(member: Member, group: Group, anchor: string, order: Map<string, number>) {
  if (member.anchor) return "This message";
  const own = member.evidence.id;
  const links = group.links.filter(l => l.a === own || l.b === own);
  const link = links.find(l => l.a === anchor || l.b === anchor) ?? links[0];
  if (!link) return "Linked report";
  const peer = link.a === own ? link.b : link.a;
  const later = (order.get(own) ?? 0) > (order.get(peer) ?? 0);
  const state = link.a === own ? link.a_state : link.b_state;
  switch (link.relation) {
    case "denial": return state === "denied" ? "Denial" : "Disputed report";
    case "update": return later ? "Update" : "Earlier report";
    case "correction": return later ? "Correction" : "Corrected report";
    default: return "Same event";
  }
}
function place(e: Evidence): [number, number, number, number] | null {
  const points = e.map_locations.map(s => s.geometry.coordinates);
  const boxes = e.map_areas.map(a => a.bounds);
  if (!points.length && !boxes.length) return null;
  const xs = [...points.map(p => p[0]), ...boxes.flatMap(b => [b[0], b[2]])];
  const ys = [...points.map(p => p[1]), ...boxes.flatMap(b => [b[1], b[3]])];
  return [Math.min(...xs), Math.min(...ys), Math.max(...xs), Math.max(...ys)];
}

type Handlers = {
  onSelect: (id: string, revision: string) => void;
  onLocate: (coordinates: [number, number] | [number, number, number, number]) => void;
};
/** The selected message's events: for each event it reports, every report
 *  connected to it through reviewed links in the current view, in publication
 *  order. Suggestions, not confirmation. */
export default function EventTimeline({ id, revision, scope, onSelect, onLocate }: { id: string; revision: string; scope: Scope } & Handlers) {
  const query = useQuery({
    queryKey: ["event-group", id, revision, scope], enabled: !!scope.publication_id,
    queryFn: ({ signal }) => api("/event-links/group", responseSchema, { method: "POST", body: { id, revision, scope }, signal }),
  });
  if (query.error) return <ErrorMessage error={query.error}/>;
  const groups = query.data?.groups ?? [];
  return <>{groups.map(group => <EventGroup key={group.claim.id} group={group} id={id} several={groups.length > 1} onSelect={onSelect} onLocate={onLocate}/>)}</>;
}
function EventGroup({ group, id, several, onSelect, onLocate }: { group: Group; id: string; several: boolean } & Handlers) {
  const [all, setAll] = useState(false);
  const order = new Map(group.members.map((m, i) => [m.evidence.id, i]));
  const anchorIndex = group.members.findIndex(m => m.anchor);
  // Keep the selected message visible when the list is shortened.
  const start = all || group.members.length <= SHOWN ? 0 : Math.min(Math.max(0, anchorIndex - 2), group.members.length - SHOWN);
  const shown = all ? group.members : group.members.slice(start, start + SHOWN);
  const hidden = group.members.length - shown.length;
  const sources = new Set(group.members.map(m => m.evidence.source)).size;
  const boxes = group.members.map(m => place(m.evidence)).filter((b): b is [number, number, number, number] => !!b);
  const bounds: [number, number, number, number] | null = boxes.length ? [Math.min(...boxes.map(b => b[0])), Math.min(...boxes.map(b => b[1])), Math.max(...boxes.map(b => b[2])), Math.max(...boxes.map(b => b[3]))] : null;
  return <section className="event-timeline" aria-label="Reports of the same event">
    <header>
      <div>
        <h3>Same event</h3>
        {several && <p className="event-timeline-which" dir="auto">{group.claim.text}</p>}
        <small>{group.members.length}{group.truncated ? "+" : ""} reports · {sources} {sources === 1 ? "source" : "sources"}</small>
      </div>
      {bounds && <button className="event-timeline-map" onClick={() => onLocate(bounds[0] === bounds[2] && bounds[1] === bounds[3] ? [bounds[0], bounds[1]] : bounds)} title="Show where these reports place the event"><Crosshair size={13}/>Map</button>}
    </header>
    <ol style={{ "--count": shown.length } as CSSProperties}>
      {start > 0 && <li className="event-timeline-more"><button onClick={() => setAll(true)}>{start} earlier</button></li>}
      {shown.map((m, i) => {
        const label = tag(m, group, id, order);
        const denied = m.claims.some(c => c.state === "denied");
        return <li key={`${m.evidence.id}@${m.evidence.revision}`} data-anchor={m.anchor || undefined} data-kind={denied ? "denial" : label === "Update" || label === "Correction" ? "update" : undefined} style={{ "--i": i } as CSSProperties}>
          <span className="event-timeline-dot" aria-hidden/>
          <button disabled={m.anchor} aria-current={m.anchor || undefined} onClick={() => onSelect(m.evidence.id, m.evidence.revision)}>
            <span className="event-timeline-meta"><MessageTime {...m.evidence}/><span className="event-timeline-tag">{label}</span></span>
            <span className="event-timeline-source" dir="auto">{m.evidence.source_label.replace(/^https?:\/\/t\.me\//, "@").replace(/^Fictional /, "")}</span>
            {!m.anchor && <span className="event-timeline-claim" dir="auto">{m.claims[0]?.text ?? readingPreview(m.evidence)}</span>}
          </button>
        </li>;
      })}
      {hidden - start > 0 && <li className="event-timeline-more"><button onClick={() => setAll(true)}>{hidden - start} more</button></li>}
    </ol>
    <p className="event-timeline-note">Linked by model review of each message's reading. Repeated reports are not independent confirmation.</p>
  </section>;
}
