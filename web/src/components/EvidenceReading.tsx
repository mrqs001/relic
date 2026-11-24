import { MapPin } from "lucide-react";
import type { EvidenceDetail } from "../api";
import { claimStatusLabel, readingPreview, placeLabel, readingUsesSummary } from "./reading-display";
import { EventLabel, PossibleEventLabel } from "./EventLabel";
import EvidenceMedia from "./EvidenceMedia";
import type { MediaRecovery } from "./EvidenceMedia";

export default function EvidenceReading({ evidence, related, onRelated, onLocate, onOriginal, onSelect, ...recovery }: { evidence: EvidenceDetail; related?: {key:string;count:number}|null; onRelated:(key:string)=>void; onLocate:(coordinates:[number,number]|[number,number,number,number])=>void; onOriginal:()=>void; onSelect:(id:string,revision:string)=>void } & MediaRecovery) {
  const reading = evidence.interpretation;
  // A warning's own position is the issuer's, and MSCIO's is deliberately inexact.
  const issued = (site: EvidenceDetail["map_locations"][number]) => !!evidence.advisory && site.id === "single";
  const locationButton = (site: EvidenceDetail["map_locations"][number]) => <button key={site.id} onClick={() => onLocate(site.geometry.coordinates)} title={issued(site) ? evidence.location_precision : site.approximate ? "Approximate reference location supported by the message" : "Location supplied by the source"}><MapPin size={13}/>{issued(site) ? `${evidence.advisory!.issuer} position` : placeLabel(site, reading)}{(site.approximate || issued(site)) && <small>Approx.</small>}</button>;
  const places = [...evidence.map_locations.map(locationButton), ...evidence.map_areas.map(area => <button key={area.id} onClick={() => onLocate(area.bounds)} title={`Reported somewhere within this publisher boundary${area.represented_time ? ` (${area.represented_time})` : ""}; exact location unknown`}><MapPin size={13}/>{placeLabel(area, reading)}<small>Area</small></button>)];
  return <section className="message-reading" aria-label="Message text">
    <div className="message-context">
      {evidence.advisory && <span className="advisory-label" title="Institutional warning. The position is the issuer's; the time is when the issuer registered it.">{evidence.advisory.issuer} warning{evidence.advisory.type ? ` · ${evidence.advisory.type}` : ""}{evidence.advisory.reference ? ` · ${evidence.advisory.reference}` : ""}</span>}
      <EventLabel reading={reading} expanded/>
      {!reading?.events.length && <PossibleEventLabel possible={evidence.possible_event}/>}
      {(reading?.translation || readingUsesSummary(evidence)) && <button onClick={onOriginal} title={readingUsesSummary(evidence) ? "Full translation is unavailable; this is the saved English summary. View the complete original." : "Machine translation; view the exact source text"}>{readingUsesSummary(evidence) ? "English summary · Original" : "Translated · Original"}</button>}
      {reading && !reading.events.length && ["denied","planned","historical"].includes(reading.claim_status) && <span>{claimStatusLabel[reading.claim_status]}</span>}
    </div>
    {!reading && evidence.text && !evidence.advisory && <small className="reading-pending">Original · no reading yet</small>}
    <p className="message-text" dir="auto">{readingPreview(evidence)}</p>
    <EvidenceMedia evidence={evidence} onSelect={onSelect} {...recovery}/>
    <div className="message-links">
      {places.slice(0, 2)}
      {places.length > 2 && <details className="message-more-sites"><summary>{places.length - 2} more places</summary><div>{places.slice(2)}</div></details>}
      {related && related.count > 0 && <button onClick={() => onRelated(related.key)} title="Same city, topic and day. Reports may share an origin.">{related.count} related ↗</button>}
    </div>
  </section>;
}
