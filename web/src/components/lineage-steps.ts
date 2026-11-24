import type { EvidenceDetail, MessageLineageData } from "../api";
import { siteLabel } from "./reading-display";

type Node = MessageLineageData["nodes"][number];
type Edge = MessageLineageData["edges"][number];
export type Reading = MessageLineageData["readings"][number];
export type StepId = "source" | "collected" | "versions" | "read" | "jev" | "located" | "feed" | "related";
/** recorded: a record exists. empty: the step ran and found nothing. gap: nothing was recorded. */
export type StepState = "recorded" | "empty" | "gap" | "advisory";
export type Step = { id: StepId; title: string; fact: string; state: StepState; stack: number; branch?: Step; records: string[] };
export type RelatedLink = { node: Node; id: string; revision: string; text: string; state: string | null };
export type Trace = {
  steps: Step[];
  nodes: Map<string, Node>;
  edges: Edge[];
  source?: Node;
  collectionRun?: Node;
  versions: Node[];
  readings: Reading[];
  /** Base reading of the displayed chain, then each geography-only update. */
  chain: Reading[];
  reference?: Node;
  publication?: Node;
  jev?: Node;
  /** False when the suggestions were made for a reading other than the one shown. */
  jevCurrent: boolean;
  related: RelatedLink[];
};

const shortFormat = new Intl.DateTimeFormat("en-GB", { day: "numeric", month: "short", hour: "2-digit", minute: "2-digit", timeZone: "UTC" });
export const shortTime = (value: unknown) => typeof value === "string" && value ? shortFormat.format(new Date(value)) : "Time not recorded";
const text = (value: unknown) => typeof value === "string" ? value : undefined;

/** "openai/gpt-6-luna" → "GPT-6 Luna"; the exact identifier stays in the record. */
export function modelName(model: string) {
  const parts: string[] = [];
  for (const part of (model.split("/").pop() || model).split("-").filter(Boolean)) {
    if (/^\d/.test(part) && parts.length) parts[parts.length - 1] += `-${part}`;
    else parts.push(part.toLowerCase() === "gpt" ? "GPT" : part[0].toUpperCase() + part.slice(1));
  }
  return parts.join(" ");
}

const providers: Record<string, string> = { telegram: "Telegram channel", x: "X account", flightradar: "Flight feed", keelwatch: "Ship feed" };
export function sourceKind(evidence: Pick<EvidenceDetail, "source">, source?: Node) {
  const provider = text(source?.detail.provider) ?? evidence.source.split(":")[0];
  return providers[provider] ?? "Imported source";
}

export function buildTrace(lineage: MessageLineageData, evidence: EvidenceDetail): Trace {
  const nodes = new Map(lineage.nodes.map(node => [node.id, node]));
  const root = lineage.root;
  const from = (id: string, relationship: string) => lineage.edges.filter(e => e.source === id && e.relationship === relationship).map(e => nodes.get(e.target)).filter((n): n is Node => !!n);
  const into = (id: string, relationship: string) => lineage.edges.filter(e => e.target === id && e.relationship === relationship).map(e => nodes.get(e.source)).filter((n): n is Node => !!n);
  const created = (node: Node) => text(node.detail.created_at) ?? "";

  const source = into(root, "acquired_from")[0];
  const producers = new Set(lineage.edges.filter(e => e.relationship === "produced").map(e => e.source));
  const collectionRun = from(root, "input_record").find(run => run.kind === "run_step" && !producers.has(run.id));
  const versions = lineage.nodes.filter(n => n.kind === "evidence_revision")
    .sort((a, b) => (text(a.detail.acquired_at) ?? "").localeCompare(text(b.detail.acquired_at) ?? "") || a.id.localeCompare(b.id));

  const readings = [...lineage.readings].sort((a, b) => a.created_at.localeCompare(b.created_at));
  const byReading = new Map(readings.map(r => [r.id, r]));
  const chain: Reading[] = [];
  for (let r = byReading.get(evidence.interpretation?.id ?? ""); r && !chain.includes(r); r = r.processing_mode === "geography_only" && r.prior_interpretation_id ? byReading.get(r.prior_interpretation_id) : undefined)
    chain.unshift(r);
  const base = chain[0], current = chain[chain.length - 1];

  const reference = current && into(current.id, "reference_used")[0];
  const publication = current
    ? from(current.id, "published").sort((a, b) => created(a).localeCompare(created(b)))[0]
    : collectionRun && from(collectionRun.id, "published")[0];
  const jev = from(root, "supplementary input")[0];
  const jevReadings = jev ? into(jev.id, "candidate context").map(n => n.id) : [];
  const jevCurrent = !!evidence.supplemental || jevReadings.some(id => chain.some(r => r.id === id));
  const related: RelatedLink[] = [];
  for (const node of current ? from(current.id, "relationship_input") : []) {
    for (const side of ["a", "b"] as const) {
      const peer = node.detail[side] as { evidence_id?: string; revision?: string; claim?: { text?: string; state?: string } } | undefined;
      if (peer?.evidence_id && peer.revision && peer.evidence_id !== evidence.id && !related.some(r => r.id === peer.evidence_id && r.revision === peer.revision))
        related.push({ node, id: peer.evidence_id, revision: peer.revision, text: peer.claim?.text ?? "", state: peer.claim?.state ?? null });
    }
  }

  const fullReadings = readings.filter(r => r.processing_mode !== "geography_only");
  const place = siteLabel(evidence) || current?.location_label || "";
  const steps: Step[] = [
    source
      ? { id: "source", title: source.label, fact: sourceKind(evidence, source), state: "recorded", stack: 1, records: [source.id] }
      : { id: "source", title: evidence.source_label, fact: "Source record missing", state: "gap", stack: 1, records: [] },
    {
      id: "collected", title: "Collected", fact: shortTime(evidence.acquired_at), state: "recorded", stack: 1,
      records: [root, ...(collectionRun ? [collectionRun.id] : [])],
      branch: versions.length > 1 ? { id: "versions", title: `${versions.length} versions`, fact: "Edited", state: "recorded", stack: 1, records: versions.map(v => v.id) } : undefined,
    },
    base
      ? { id: "read", title: "Read", fact: `${modelName(base.model)} · ${fullReadings.length > 1 ? `${fullReadings.length} readings` : shortTime(base.created_at)}`, state: "recorded", stack: fullReadings.length, records: [...fullReadings.map(r => r.id), ...into(base.id, "produced").map(n => n.id)] }
      : { id: "read", title: "Read", fact: !evidence.text.trim() ? "No text to read" : readings.length ? "No reading in this view" : "Not read yet", state: "gap", stack: 1, records: [] },
    base
      ? { id: "located", title: place ? "Located" : "Not located", fact: `${place || "No place resolved"}${chain.length > 1 ? ` · ${chain.length - 1} ${chain.length === 2 ? "update" : "updates"}` : ""}`, state: place ? "recorded" : "empty", stack: chain.length, records: [...chain.slice(chain.length > 1 ? 1 : 0).map(r => r.id), ...(reference ? [reference.id] : [])] }
      : { id: "located", title: "Located", fact: "Needs a reading", state: "gap", stack: 1, records: [] },
    publication
      ? { id: "feed", title: "In the feed", fact: shortTime(publication.detail.created_at), state: "recorded", stack: 1, records: [publication.id] }
      : { id: "feed", title: "In the feed", fact: "Publication not recorded", state: "gap", stack: 1, records: [] },
  ];
  if (jev || evidence.supplemental)
    steps[2].branch = { id: "jev", title: "Jev", fact: jevCurrent ? "Suggestions" : "Earlier reading", state: "advisory", stack: 1, records: jev ? [jev.id] : [] };
  if (related.length)
    steps[4].branch = { id: "related", title: `${related.length} related`, fact: "Suggested", state: "recorded", stack: 1, records: [...new Set(related.map(r => r.node.id))] };
  return { steps, nodes, edges: lineage.edges, source, collectionRun, versions, readings, chain, reference, publication, jev, jevCurrent, related };
}

export function findStep(trace: Trace, id: string) {
  for (const step of trace.steps) {
    if (step.id === id) return step;
    if (step.branch?.id === id) return step.branch;
  }
  return undefined;
}
