import { useEffect, useMemo, useState } from "react";
import type { CSSProperties, KeyboardEvent, ReactNode } from "react";
import { useQuery } from "@tanstack/react-query";
import { api, date, label, messageLineageSchema } from "../api";
import type { Citation, EvidenceDetail } from "../api";
import { Busy, ErrorMessage } from "./Common";
import { eventLabels } from "./EventLabel";
import { buildTrace, findStep, modelName, shortTime, sourceKind } from "./lineage-steps";
import type { Step, StepId, Trace } from "./lineage-steps";
import { shortRevision, siteLabel } from "./reading-display";
import "../message-lineage.css";

type Props = {
  evidence: EvidenceDetail;
  onSelect: (id: string, revision: string) => void;
  onLocate: (coordinates: [number, number] | [number, number, number, number]) => void;
  onPassage: (citation: Citation | null) => void;
};

export default function MessageLineage({ evidence, onSelect, onLocate, onPassage }: Props) {
  const root = evidence.lineage_root;
  const query = useQuery({
    queryKey: ["lineage", "message", root],
    queryFn: ({ signal }) => api(`/lineage/message?root=${encodeURIComponent(root)}`, messageLineageSchema, { signal }),
  });
  const trace = useMemo(() => query.data && buildTrace(query.data, evidence), [query.data, evidence]);
  const [selected, setSelected] = useState<StepId | null>(null);
  // The first card waits for the line to finish drawing; later picks open at once.
  const [picked, setPicked] = useState(false);
  useEffect(() => {
    if (trace && (!selected || !findStep(trace, selected)))
      setSelected(trace.steps[2].state === "recorded" ? "read" : "collected");
  }, [trace]);
  const step = trace && selected ? findStep(trace, selected) : undefined;
  return <section className="lineage" aria-label="Message lineage" style={{ "--steps": trace?.steps.length ?? 5, ...(picked ? { "--card-delay": "0ms" } : {}) } as CSSProperties}>
    <ErrorMessage error={query.error}/>{query.isLoading && <Busy/>}
    {trace && <>
      <TraceView trace={trace} selected={selected} onSelect={id => { setPicked(true); setSelected(id); }}/>
      {step && <StepCard key={step.id} step={step} trace={trace} evidence={evidence} onSelect={onSelect} onLocate={onLocate} onPassage={onPassage}/>}
      {query.data!.hidden_nodes > 0 && <p className="lineage-limit">{query.data!.hidden_nodes}+ further records are not shown.</p>}
    </>}
  </section>;
}

function TraceView({ trace, selected, onSelect }: { trace: Trace; selected: StepId | null; onSelect: (id: StepId) => void }) {
  // Up/down follows the main line; right/left moves between a step and its branch.
  const move = (event: KeyboardEvent<HTMLOListElement>) => {
    const target = event.target as HTMLElement;
    const row = target.closest("li");
    const rows = [...event.currentTarget.children] as HTMLElement[];
    const index = row ? rows.indexOf(row) : -1;
    const focus = (el: Element | null | undefined) => { if (el instanceof HTMLElement) { event.preventDefault(); el.focus(); } };
    if (event.key === "ArrowDown") focus(rows[index + 1]?.querySelector(".trace-node"));
    else if (event.key === "ArrowUp") focus(rows[index - 1]?.querySelector(".trace-node"));
    else if (event.key === "ArrowRight") focus(row?.querySelector(".trace-branch"));
    else if (event.key === "ArrowLeft") focus(row?.querySelector(".trace-node"));
    else if (event.key === "Home") focus(rows[0]?.querySelector(".trace-node"));
    else if (event.key === "End") focus(rows[rows.length - 1]?.querySelector(".trace-node"));
  };
  return <div className="trace"><ol aria-label="Steps this message went through" onKeyDown={move}>
    {trace.steps.map((step, index) => <li key={step.id} className="trace-row" data-state={step.state} data-next={trace.steps[index + 1]?.state} style={{ "--i": index } as CSSProperties}>
      <button className="trace-node" aria-current={selected === step.id ? "step" : undefined} aria-controls="lineage-step" onClick={() => onSelect(step.id)}>
        <span className="trace-ring" data-stack={Math.min(step.stack, 3)} aria-hidden/>
        <span className="trace-text"><strong dir="auto">{step.title}</strong><small dir="auto">{step.fact}</small></span>
      </button>
      {step.branch && <>
        <span className="trace-wire" data-state={step.branch.state} aria-hidden><i/><svg viewBox="0 0 34 30" width="34" height="30">
          <path d="M0 9 C 18 9, 16 25, 34 25" pathLength={1}/>
        </svg></span>
        <button className="trace-branch" data-state={step.branch.state} aria-current={selected === step.branch.id ? "step" : undefined} aria-controls="lineage-step" onClick={() => onSelect(step.branch!.id)}>
          <strong>{step.branch.title}</strong><small>{step.branch.fact}</small>
        </button>
      </>}
    </li>)}
  </ol><span className="trace-pulse" aria-hidden/></div>;
}

type CardProps = { step: Step; trace: Trace; evidence: EvidenceDetail } & Omit<Props, "evidence">;
function StepCard(props: CardProps) {
  const { step, trace } = props;
  const Body = cards[step.id];
  const records = step.records.map(id => trace.nodes.get(id)).filter(Boolean);
  // How this step's records were derived: edges into them from records on the path.
  const own = new Set(step.records);
  const path = new Set(trace.steps.flatMap(s => [...s.records, ...(s.branch?.records ?? [])]));
  const edges = trace.edges.filter(e => own.has(e.target) && path.has(e.source));
  const [open, setOpen] = useState(false);
  return <section id="lineage-step" className="lineage-card" data-state={step.state} aria-live="polite" aria-label={step.title}>
    <Body {...props}/>
    {records.length > 0 && <footer>
      <button className="lineage-record-toggle" aria-expanded={open} onClick={() => setOpen(!open)}>{open ? "Hide exact record" : "Exact record"}</button>
      {open && <pre>{JSON.stringify({ nodes: records, edges }, null, 2)}</pre>}
    </footer>}
  </section>;
}

function Head({ title, sub }: { title: ReactNode; sub?: ReactNode }) {
  return <header><h3 dir="auto">{title}</h3>{sub && <p>{sub}</p>}</header>;
}
function Facts({ rows }: { rows: [string, ReactNode | undefined | null][] }) {
  const shown = rows.filter(([, value]) => value !== undefined && value !== null && value !== "");
  return shown.length ? <dl className="lineage-facts">{shown.map(([name, value]) => <div key={name}><dt>{name}</dt><dd>{value}</dd></div>)}</dl> : null;
}
const languages = new Intl.DisplayNames(["en"], { type: "language" });
function languageName(code: string | null | undefined) {
  if (!code || code === "und") return undefined;
  if (code === "mul") return "Several languages";
  try { return languages.of(code) ?? code; } catch { return code; }
}
const text = (value: unknown) => typeof value === "string" ? value : undefined;
const sentence = (value: string) => value.charAt(0).toUpperCase() + value.slice(1);

const cards: Record<StepId, (props: CardProps) => ReactNode> = {
  source: ({ trace, evidence }) => {
    const detail = trace.source?.detail ?? {};
    return <>
      <Head title={trace.source?.label ?? evidence.source_label} sub={trace.source ? sourceKind(evidence, trace.source) : "No source record was retained for this message."}/>
      <Facts rows={[["Status", text(detail.status) && sentence(label(text(detail.status)!))], ["Last collected", text(detail.last_acquired_at) && date(text(detail.last_acquired_at))]]}/>
      {text(detail.coverage) && <p className="lineage-note">{text(detail.coverage)}</p>}
    </>;
  },
  collected: ({ trace, evidence, onPassage }) => {
    const index = trace.versions.findIndex(v => v.id === evidence.lineage_root);
    return <>
      <Head title="Collected" sub={date(evidence.acquired_at)}/>
      <Facts rows={[
        ["At source", evidence.upstream_state === "removed" ? "Removed at source" : evidence.published_at ? date(evidence.published_at) : "Time not supplied"],
        ["Version", <>{shortRevision(evidence.revision)}{trace.versions.length > 1 && index >= 0 && <span className="lineage-quiet"> · {index + 1} of {trace.versions.length}</span>}</>],
        ["Run", trace.collectionRun && trace.collectionRun.label],
      ]}/>
      <div className="lineage-actions">
        <button onClick={() => onPassage(null)}>View original</button>
        {evidence.original_input_hashes?.map((hash, i) => <a key={hash} href={`/api/v1/objects/${encodeURIComponent(hash)}`} download>Source record{evidence.original_input_hashes!.length > 1 ? ` ${i + 1}` : ""}</a>)}
      </div>
    </>;
  },
  versions: ({ trace, evidence, onSelect }) => <>
    <Head title="Versions" sub="The source edited this message. Every version is kept."/>
    <ol className="lineage-rows">{trace.versions.map((version, i) => {
      const current = version.id === evidence.lineage_root;
      const id = text(version.detail.evidence_id), revision = text(version.detail.revision);
      return <li key={version.id}>
        <button disabled={current || !id || !revision} aria-current={current ? "true" : undefined} onClick={() => onSelect(id!, revision!)}>
          <strong>Version {i + 1}</strong><span>{shortTime(version.detail.acquired_at)}{current ? " · shown" : ""}</span>
        </button>
      </li>;
    })}</ol>
  </>,
  read: ({ trace, evidence, onPassage }) => {
    const [all, setAll] = useState(false);
    const base = trace.chain[0];
    if (!base) return <Head title="Not read" sub={!evidence.text.trim()
      ? "This version has no text to read."
      : trace.readings.length ? "Readings of this version exist, but none belongs to the feed publication shown." : "No reading has been recorded for this version."}/>;
    const full = trace.readings.filter(r => r.processing_mode !== "geography_only");
    const claims = evidence.interpretation?.claims ?? [];
    return <>
      <Head title={`Read by ${modelName(base.model)}`} sub={date(base.created_at)}/>
      <Facts rows={[["Model", base.model], ["Language", languageName(base.language)], ["Statements", claims.length || "None extracted"]]}/>
      {claims.length > 0 && <ol className="lineage-statements">{claims.slice(0, all ? undefined : 3).map((claim, i) => <li key={i}>
        <p dir="auto">{claim.text}</p>
        <button onClick={() => onPassage(claim.citation)}>Show passage</button>
      </li>)}</ol>}
      {claims.length > 3 && !all && <div className="lineage-actions"><button onClick={() => setAll(true)}>Show {claims.length - 3} more</button></div>}
      {full.length > 1 && <p className="lineage-note">Read {full.length} times. The reading from {shortTime(base.created_at)} is shown; earlier ones stay in the record.</p>}
    </>;
  },
  jev: ({ trace, evidence, onSelect }) => {
    // The message detail carries suggestions only for the reading shown; the
    // lineage record keeps the answers of suggestions made for earlier readings.
    const data = evidence.supplemental;
    const record = trace.jev?.detail ?? {};
    const answers = (data?.answers ?? record.answers ?? {}) as Record<string, { choice: string }>;
    const request = data?.request_hash ?? text(record.request_hash), audit = data?.audit_hash ?? text(record.audit_hash);
    const describe = (key: string) => sentence(label(answers[key]?.choice ?? "uncertain"));
    const types = Object.entries(answers).filter(([key, a]) => key.startsWith("event_") && !["absent", "uncertain"].includes(a.choice))
      .map(([key]) => eventLabels[key.slice(6) as keyof typeof eventLabels] || label(key.slice(6)));
    return <>
      <Head title="Jev" sub={trace.jevCurrent
        ? `Advisory suggestions${data ? ` from ${shortTime(data.created_at)}` : ""}. The reading and map are unchanged.`
        : "Advisory suggestions made for an earlier reading. The reading and map are unchanged."}/>
      {Object.keys(answers).length > 0 && <Facts rows={[["Message", describe("kind")], ["Claim", describe("claim_state")], ["Priority", describe("priority")], ["Translation", describe("translation")], ["Context", describe("context")], ["Types", types.join(", ")]]}/>}
      {data && data.context.neighbours.length > 0 && <ol className="lineage-rows">{data.context.neighbours.map((n, i) =>
        <li key={`${n.id}@${n.revision}`}><button onClick={() => onSelect(n.id, n.revision)}><strong>Nearby message {i + 1}</strong><span>{describe(`link_${i}`)}</span></button></li>)}</ol>}
      {data && data.context.locations.length > 0 && <Facts rows={data.context.locations.map((loc, i) => {
        const choice = data.answers[`geo_${i}`]?.choice;
        const candidate = choice?.startsWith("candidate_") ? loc.candidates[Number(choice.slice(10))] : undefined;
        return [loc.mention, candidate ? `${candidate.name}${candidate.country_code ? ` (${candidate.country_code})` : ""}` : describe(`geo_${i}`)];
      })}/>}
      {data && (data.context.omitted_neighbours > 0 || data.context.omitted_locations > 0) && <p className="lineage-note">Some context exceeded this request's limit.</p>}
      {(request || audit) && <div className="lineage-actions">
        {request && <a href={`/api/v1/objects/${request}`} download>Exact inputs</a>}
        {audit && <a href={`/api/v1/objects/${audit}`} download>Response receipt</a>}
      </div>}
    </>;
  },
  located: ({ trace, evidence, onLocate }) => {
    const place = siteLabel(evidence) || trace.chain[trace.chain.length - 1]?.location_label;
    const reference = trace.reference;
    const edition = text(reference?.detail.edition) ?? reference?.id.split(":").pop();
    const site = evidence.map_locations[0], area = evidence.map_areas[0];
    const kind = area ? `Area${area.represented_time ? ` · ${area.represented_time}` : ""}` : site ? site.approximate ? "Approximate location" : "Location supplied by the source" : undefined;
    const updates = trace.chain.length - 1;
    return <>
      <Head title={place || "Not located"} sub={place ? kind : "No place could be resolved from the message text."}/>
      <Facts rows={[
        ["Reference", reference?.label],
        ["Edition", edition && shortRevision(edition)],
        ["Updates", updates > 0 ? `${updates} as reference data changed, last ${shortTime(trace.chain[updates].created_at)}` : undefined],
      ]}/>
      {(site || area) && <div className="lineage-actions"><button onClick={() => onLocate(site ? site.geometry.coordinates : area.bounds)}>Show on map</button></div>}
    </>;
  },
  feed: ({ trace }) => {
    const publication = trace.publication;
    if (!publication) return <Head title="In the feed" sub="No publication record links this message to the feed."/>;
    return <>
      <Head title="In the feed" sub={date(text(publication.detail.created_at))}/>
      <Facts rows={[["Publication", publication.label], ["Status", text(publication.detail.status) && sentence(label(text(publication.detail.status)!))]]}/>
      {(text(publication.detail.meaning) ?? text(publication.detail.complete_meaning)) && <p className="lineage-note">{text(publication.detail.meaning) ?? text(publication.detail.complete_meaning)}</p>}
    </>;
  },
  related: ({ trace, onSelect }) => <>
    <Head title="Related messages" sub="Matched on the same event. A match is not independent confirmation."/>
    <ol className="lineage-rows">{trace.related.map(r => <li key={`${r.id}@${r.revision}`}>
      <button onClick={() => onSelect(r.id, r.revision)}><strong dir="auto">{r.text || "Related message"}</strong><span>{r.state ? sentence(label(r.state)) : "Open message"}</span></button>
    </li>)}</ol>
  </>,
};
