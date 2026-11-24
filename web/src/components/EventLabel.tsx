import type { EventType, Interpretation } from "../api";
import { claimStatusLabel } from "./reading-display";

import taxonomy from "../event-taxonomy.json";
export const eventGroups = [...new Set(taxonomy.types.map(t => t.group))];
export const eventLabels = Object.fromEntries(taxonomy.types.map(t => [t.id, t.label])) as Record<EventType, string>;
export const eventGroup = Object.fromEntries(taxonomy.types.map(t => [t.id, t.group])) as Record<EventType, string>;
// One distinct colour per event group, shared by map markers and feed rows so
// the list and the map read as one picture.
export const groupColors: Record<string, string> = {Conflict: "#f0705c", "Military activity": "#8fa6ff", Safety: "#f2aa3c", Infrastructure: "#e3cf55", "Politics and diplomacy": "#4cc6cf", "Civil affairs": "#c690ef", Other: "#8fb9a6"};
export const eventColors = Object.fromEntries(taxonomy.types.map(t => [t.id, groupColors[t.group]])) as Record<EventType, string>;
type ReadingEvents = {events: Pick<Interpretation["events"][number], "type" | "state">[]} | null | undefined;
/** The distinct event kinds of a reading and the one to show first: a reported
 *  event of the chosen type, else any of that type, else the first. */
export function primaryEvent(reading: ReadingEvents, preferred?: EventType | null) {
  const events = reading?.events ?? [];
  const kinds = [...new Map(events.map(event => [`${event.type}/${event.state}`, event])).values()];
  const primary = kinds.find(e => (!preferred || e.type === preferred) && e.state === "reported") || kinds.find(e => e.type === preferred) || kinds[0];
  return { primary, kinds };
}
type Possible = {type: EventType; confidence: number} | null | undefined;
/** Why a possible event is shown, in full: who suggested it and what it is not. */
export const possibleTitle = (possible: NonNullable<Possible>) =>
  `Possible ${eventLabels[possible.type].toLowerCase()}, suggested by Jev, a second model (confidence ${Math.round(possible.confidence * 100)}%). The main reading found no event, so this is not a reviewed reading.`;
/** Jev's suggestion where the main reading found no event: a hollow mark. */
export function PossibleEventLabel({possible}: {possible: Possible}) {
  if (!possible) return null;
  return <span className="event-label possible" title={possibleTitle(possible)}>
    <i style={{borderColor: eventColors[possible.type]}}/>{eventLabels[possible.type]}<small>possible</small>
  </span>;
}
export function EventLabel({reading, preferred, expanded = false}: {reading?: ReadingEvents; preferred?: EventType | null; expanded?: boolean}) {
  const { primary, kinds } = primaryEvent(reading, preferred);
  if (!primary) return null;
  if (expanded && kinds.length > 1) return <span className="event-labels">{kinds.map(event => <EventLabel key={`${event.type}/${event.state}`} reading={{events:[event]}}/>)}</span>;
  return <span className="event-label" title={`Source claims, inferred from the text: ${kinds.map(e => `${eventLabels[e.type]} (${claimStatusLabel[e.state].toLowerCase()})`).join(", ")}`}>
    <i style={{background: eventColors[primary.type]}}/>{eventLabels[primary.type]}{primary.state !== "reported" && ` · ${claimStatusLabel[primary.state]}`}{kinds.length > 1 && <small>+{kinds.length - 1}</small>}
  </span>;
}
