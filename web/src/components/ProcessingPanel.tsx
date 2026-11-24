import { useEffect, useId, useState } from "react";
import type { CSSProperties } from "react";
import { queryOptions, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Pause, Play } from "lucide-react";
import { z } from "zod";
import { api, jobSchema, label, processingSchema } from "../api";
import type { Job, ProcessingStatus, Workspace } from "../api";
import { ErrorMessage } from "./Common";
import ProcessingFailures from "./ProcessingFailures";
import { modelName } from "./lineage-steps";
import "../processing-panel.css";

const number = (n: number) => n.toLocaleString("en-GB");
/** Enough digits that a non-zero amount never reads as $0.000: ship-finder
 *  requests cost about a hundredth of a cent. */
const money = (n: number) => n <= 0 ? "$0.00" : n < 0.0001 ? "<$0.0001" : `$${n.toFixed(n < 0.01 ? 4 : n < 0.1 ? 3 : 2)}`;
/** Known spend, plus requests whose cost the provider never reported. */
const spend = (known: number, unknown: number, label = "") => `${money(known)}${label}${unknown ? ` + ${number(unknown)} unknown` : ""}`;
const unknownHint = "Requests whose cost the provider did not report. They may have been charged.";
const clock = new Intl.DateTimeFormat("en-GB", { hour: "2-digit", minute: "2-digit", timeZone: "UTC" });
const day = new Intl.DateTimeFormat("en-GB", { day: "numeric", month: "short", timeZone: "UTC" });
const full = new Intl.DateTimeFormat("en-GB", { day: "numeric", month: "short", hour: "2-digit", minute: "2-digit", timeZone: "UTC" });
// Times are UTC like the rest of the app; older activity shows only its date.
const today = (value: string) => value.slice(0, 10) === new Date().toISOString().slice(0, 10);
const when = (value: string) => today(value) ? clock.format(new Date(value)) : day.format(new Date(value));

/** The one processing status query. Processing and Sources share this cache
 *  entry, so one request serves both views. */
export const processingQuery = queryOptions({
  queryKey: ["processing"], queryFn: ({ signal }) => api("/processing", processingSchema, { signal }),
});

export function ProcessingPanel({ workspace }: { workspace: Workspace }) {
  const client = useQueryClient();
  const [stopRequested, setStopRequested] = useState(false);
  // Publication, processing and Jev events refresh the status promptly (see
  // App); this slower poll covers progress that emits no event.
  const processing = useQuery({
    ...processingQuery,
    refetchInterval: query => query.state.data?.running ? 4000 : query.state.data?.policy.enabled ? 5000 : false,
    refetchIntervalInBackground: false,
  });
  const data = processing.data;
  useEffect(() => { if (data && !data.running) setStopRequested(false); }, [data]);
  const action = useMutation({
    mutationFn: async (kind: "run" | "pause") => {
      if (!data) throw new Error("Processing status is unavailable.");
      if (kind === "pause") {
        const policy = await api("/processing/policy", processingSchema, { method: "PUT", body: { ...data.policy, enabled: false } });
        client.setQueryData(["processing"], policy);
        const stopped = await api("/processing/stop", processingSchema, { method: "POST", body: {} });
        setStopRequested(stopped.running);
        return stopped;
      }
      setStopRequested(false);
      return api("/processing/policy", processingSchema, { method: "PUT", body: { ...data.policy, enabled: true } });
    },
    onSuccess: value => {
      client.setQueryData(["processing"], value);
      for (const key of ["processing", "workspace", "evidence-query", "evidence"]) void client.invalidateQueries({ queryKey: [key] });
    },
    onError: () => { void client.invalidateQueries({ queryKey: ["processing"] }); },
  });
  const state = status(data, stopRequested);
  const canResume = !!data && data.configured && data.evaluated && data.policy.eligible_source_ids.length > 0 && !action.isPending;
  return <section className="processing-view" aria-label="Processing">
    <header className="processing-state" data-tone={state.tone}>
      <span className="processing-beacon" aria-hidden/>
      <div><strong>{state.title}</strong>{state.detail && <span>{state.detail}</span>}</div>
      {data?.policy.enabled || data?.running
        ? <button className="button" disabled={action.isPending || stopRequested} onClick={() => action.mutate("pause")}><Pause size={13}/>{stopRequested ? "Stopping…" : "Pause"}</button>
        : <button className="button primary" disabled={!canResume} onClick={() => action.mutate("run")}><Play size={13}/>{action.isPending ? "Starting…" : "Resume"}</button>}
    </header>
    <ErrorMessage error={processing.error}/><ErrorMessage error={action.error}/>
    {data && <>
      <Coverage data={data}/>
      <Lanes data={data}/>
    </>}
    <Activity jobs={workspace.jobs}/>
  </section>;
}

function status(data: ProcessingStatus | undefined, stopping: boolean): { title: string; detail?: string; tone: "live" | "idle" | "paused" | "attention" } {
  if (!data) return { title: "Checking…", tone: "idle" };
  if (!data.configured) return { title: "Reading is not configured", detail: "Add the model provider key to the private configuration.", tone: "attention" };
  if (!data.evaluated) return { title: data.quality_notice ? "Quality review required" : "Evaluation required", detail: data.quality_notice ?? "Reading starts after the model evaluation passes.", tone: "attention" };
  if (stopping) return { title: "Stopping…", detail: "Requests already sent finish and are recorded.", tone: "paused" };
  if (data.running) return { title: data.in_flight ? `Reading ${number(data.in_flight)} ${data.in_flight === 1 ? "message" : "messages"}…` : "Reading…", detail: data.queued ? `${number(data.queued)} more waiting` : undefined, tone: "live" };
  if (!data.policy.eligible_source_ids.length) return { title: "No sources selected for reading", detail: "Choose sources to read in Sources › All sources.", tone: "attention" };
  if (!data.policy.enabled) return { title: "Paused", detail: "New messages are collected but not read.", tone: "paused" };
  if (data.retry_at && Date.parse(data.retry_at) > Date.now()) {
    const at = `${today(data.retry_at) ? clock.format(new Date(data.retry_at)) : full.format(new Date(data.retry_at))} UTC`;
    // Nothing was charged; waiting messages are read by themselves once credits return.
    return data.retry_reason === "credits"
      ? { title: "OpenRouter credits exhausted", detail: `Add credits on OpenRouter; reading retries at ${at}`, tone: "attention" }
      : { title: "Rate limited", detail: `Retrying at ${at}`, tone: "attention" };
  }
  return { title: "Automatic", detail: data.queued ? `${number(data.queued)} waiting` : "Up to date", tone: "idle" };
}

function Coverage({ data }: { data: ProcessingStatus }) {
  const client = useQueryClient();
  const [review, setReview] = useState(false);
  // Interrupted requests may already be billed, so they never retry by themselves.
  const retry = useMutation({
    mutationFn: () => api("/processing/retry-interrupted", processingSchema, { method: "POST", body: {} }),
    onSuccess: value => { client.setQueryData(["processing"], value); void client.invalidateQueries({ queryKey: ["processing-failures"] }); },
  });
  const total = data.processed + data.pending;
  const u = data.unavailable;
  const waiting = Math.max(0, data.pending - u.without_text - u.removed - u.ineligible - u.oversized);
  const segments = [
    { key: "read", name: "Read", n: data.processed },
    { key: "waiting", name: "Waiting", n: waiting },
    { key: "text", name: "No text", n: u.without_text, hint: "Media-only or empty messages" },
    { key: "removed", name: "Removed", n: u.removed, hint: "Removed at source; originals are kept" },
    { key: "ineligible", name: "Not selected", n: u.ineligible, hint: "Their source is not enabled for reading" },
    { key: "oversized", name: "Too long", n: u.oversized, hint: "Over the per-request text size; no truncated reading is sent" },
  ].filter(s => s.n > 0);
  return <section className="processing-coverage" aria-label="Reading coverage">
    <p className="processing-count"><strong>{number(data.processed)}</strong> of {number(total)} messages read</p>
    <div className="processing-bar" role="img" aria-label={segments.map(s => `${s.name} ${s.n}`).join(", ")}>
      {segments.map(s => <span key={s.key} data-segment={s.key} style={{ "--share": total ? s.n / total : 0 } as CSSProperties}/>)}
    </div>
    <ul className="processing-legend">
      {segments.map(s => <li key={s.key} data-segment={s.key} title={s.hint}><i/>{s.name}<b>{number(s.n)}</b></li>)}
    </ul>
    {(data.failed > 0 || data.needs_upgrade > 0 || data.corrections_queued > 0) && <div className="processing-notes">
      {data.failed > 0 && <button className="processing-note attention" aria-expanded={review} title={data.last_error ? `Latest: ${data.last_error}` : undefined} onClick={() => setReview(!review)}>{number(data.failed)} {data.failed === 1 ? "message needs" : "messages need"} review</button>}
      {data.interrupted > 0 && <button className="processing-note action" disabled={retry.isPending} title="These requests were cut off, usually by a restart. The provider may have charged for them, so retrying sends one new paid request each." onClick={() => retry.mutate()}>{retry.isPending ? "Retrying…" : `Retry ${number(data.interrupted)} interrupted`}</button>}
      {data.needs_upgrade > 0 && <span className="processing-note">{number(data.needs_upgrade)} being updated · previous reading kept</span>}
      {data.corrections_queued > 0 && <span className="processing-note">{number(data.corrections_queued)} queued for one correction</span>}
    </div>}
    <ErrorMessage error={retry.error}/>
    {review && <ProcessingFailures/>}
  </section>;
}

function Lanes({ data }: { data: ProcessingStatus }) {
  const jev = data.supplemental, links = data.relationships;
  const unknownReading = data.today.unknown_cost_requests;
  const readingCost = data.today.cost_usd === null ? `${number(unknownReading)} ${unknownReading === 1 ? "request" : "requests"} of unknown cost` : money(data.today.cost_usd);
  return <section className="processing-lanes" aria-label="Processing lanes">
    <div className="processing-lane" data-state={data.running ? "live" : data.policy.enabled ? "on" : "off"}>
      <span className="lane-ring" aria-hidden/>
      <div><strong>Reading</strong><small>Translation, statements and places for every message with text</small></div>
      <span className="lane-usage">{number(data.today.calls)} today · {readingCost}</span>
    </div>
    {jev && <div className="processing-lane branch advisory" data-state={!jev.enabled ? "off" : jev.running ? "live" : data.policy.enabled ? "on" : "off"}>
      <span className="lane-ring" aria-hidden/>
      <div>
        <strong>Jev{jev.model && <em>{modelName(jev.model)}</em>}</strong>
        <small>{jev.enabled ? `Advisory suggestions · ${number(jev.completed)} made` : "Off"}</small>
        {(jev.text_size_excluded + jev.request_excluded) > 0 && <small>{number(jev.text_size_excluded + jev.request_excluded)} messages exceed its limits; reading is unaffected.</small>}
        {jev.last_error && <small className="lane-error">{jev.last_error}</small>}
      </div>
      <span className="lane-usage" title={jev.today.unknown_cost_requests ? unknownHint : undefined}>{number(jev.today.calls)} today · {spend(jev.today.known_cost_usd, jev.today.unknown_cost_requests)}</span>
    </div>}
    {links && <LinkLane links={links}/>}
    <VesselLane paused={!data.policy.enabled}/>
  </section>;
}

const vesselSchema = z.object({
  flagged: z.number(), waiting: z.number(), auto: z.boolean(), model: z.string(),
  // Automatic mode reads only messages acquired after auto_since; it pauses
  // itself (with a reason) after a run stops early. Older servers omit these.
  auto_since: z.string().nullish(), auto_paused_reason: z.string().nullish(), auto_waiting: z.number().nullish(),
  totals: z.object({ attempted: z.number(), completed: z.number(), failed: z.number(), known_cost_usd: z.number(), unknown_cost_requests: z.number(), suggested: z.number(), confirmed: z.number() }),
  run: z.object({ running: z.boolean(), automatic: z.boolean(), planned: z.number(), done: z.number(), found: z.number(), failed: z.number(), cost_usd: z.number(), unknown_cost_requests: z.number(), stop_reason: z.string().nullish(), finished_at: z.string().nullish(), stop_requested: z.boolean().nullish() }),
  // Runs and automatic mode need a passed evaluation of the current model
  // profile; `evaluation` is the installed one, which may be for an older profile.
  evaluated: z.boolean().default(false),
  evaluation: z.object({ passed: z.boolean(), current: z.boolean().default(false), checked_at: z.string().nullish(), cases: z.number().nullish(),
    summary: z.object({ expected_vessels: z.number(), matched_vessels: z.number(), precision: z.number(), negative_vessels: z.number(), identifier_errors: z.number() }).partial().nullish() }).nullish(),
});
/** What the installed evaluation showed, in one line. */
function evaluationText(e: NonNullable<z.infer<typeof vesselSchema>["evaluation"]>) {
  const s = e.summary ?? {};
  const found = s.matched_vessels != null && s.expected_vessels != null ? `found ${number(s.matched_vessels)} of ${number(s.expected_vessels)} named ships` : null;
  const precise = s.precision != null ? `${Math.round(s.precision * 100)}% of listed ships correct` : null;
  const invented = s.negative_vessels == null ? null : s.negative_vessels ? `${number(s.negative_vessels)} invented where none was named` : "none invented where none was named";
  const ids = s.identifier_errors == null ? null : s.identifier_errors ? `${number(s.identifier_errors)} identifier errors` : "identifiers exact";
  return `Evaluated${e.checked_at ? ` ${day.format(new Date(e.checked_at))}` : ""} on ${number(e.cases ?? 0)} fictional messages: ${[found, precise, invented, ids].filter(Boolean).join(", ")}.`;
}
const utc = (value: string) => `${today(value) ? clock.format(new Date(value)) : full.format(new Date(value))} UTC`;
/** The ship finder: one paid extraction per flagged message, free KeelWatch
 *  matching. The backlog runs only on request; automatic mode reads only
 *  messages that arrive after it is switched on. */
function VesselLane({ paused }: { paused: boolean }) {
  const client = useQueryClient();
  const noteId = useId();
  const [stopSent, setStopSent] = useState(false);
  const status = useQuery({ queryKey: ["processing-vessels"], queryFn: ({ signal }) => api("/processing/vessels", vesselSchema, { signal }),
    refetchInterval: q => q.state.data?.run.running ? 2000 : 15_000 });
  const settle = (value: z.infer<typeof vesselSchema>) => { client.setQueryData(["processing-vessels"], value); void client.invalidateQueries({ queryKey: ["vessel-marks"] }); };
  const act = useMutation({ mutationFn: (action: "start" | "stop") => api("/processing/vessels", vesselSchema, { method: "POST", body: { action } }), onSuccess: settle,
    onError: (_error, action) => { if (action === "stop") setStopSent(false); } });
  const auto = useMutation({ mutationFn: (on: boolean) => api("/processing/vessels", vesselSchema, { method: "PUT", body: { auto: on } }), onSuccess: settle });
  const d = status.data;
  const running = !!d?.run.running;
  useEffect(() => { if (!running) setStopSent(false); }, [running]);
  if (!d) return status.error ? <div className="processing-lane branch" data-state="attention">
    <span className="lane-ring" aria-hidden/>
    <div><strong>Ship finder</strong><small className="lane-error">Status unavailable: {status.error instanceof Error ? status.error.message : String(status.error)}</small></div>
  </div> : null;
  const run = d.run;
  // Requests already sent (up to eight) finish and are recorded after a stop.
  const stopping = run.running && (!!run.stop_requested || stopSent);
  const autoPaused = d.auto_paused_reason || null;
  const autoOn = d.auto && !autoPaused;
  const newWaiting = d.auto_waiting == null ? null : `${number(d.auto_waiting)} new ${d.auto_waiting === 1 ? "message" : "messages"} waiting`;
  const autoText = autoPaused ? `Automatic mode paused itself: ${autoPaused}${newWaiting ? ` (${newWaiting})` : ""}`
    : autoOn ? `Automatic${d.auto_since ? ` since ${utc(d.auto_since)}` : ""}${newWaiting ? ` · ${newWaiting}` : ""}. It reads only messages that arrive after it was switched on, every two minutes, one model request each.${paused ? " It waits while processing is paused." : ""}`
    : "Automatic mode is off. When on, it reads only messages that arrive after you switch it on.";
  const unevaluated = "Evaluation required: the current model profile has not passed the ship finder evaluation, so no request is sent.";
  return <div className="processing-lane branch" data-state={run.running ? "live" : !d.evaluated || autoPaused || (run.stop_reason && run.stop_reason !== "Stopped") ? "attention" : autoOn && !paused ? "on" : "off"}>
    <span className="lane-ring" aria-hidden/>
    <div>
      <strong>Ship finder<em>{modelName(d.model)}</em></strong>
      <small>{number(d.flagged)} messages may name ships · {number(d.totals.completed)} checked · {number(d.totals.suggested)} {d.totals.suggested === 1 ? "ship" : "ships"} suggested · {number(d.totals.confirmed)} confirmed</small>
      {d.evaluated && d.evaluation && <small>{evaluationText(d.evaluation)}</small>}
      {!d.evaluated && <small className="lane-error" id={noteId}>{unevaluated}{d.evaluation && !d.evaluation.current ? " The installed evaluation is for an earlier model profile." : ""} Ships already found stay readable.</small>}
      {run.running && <small>{run.automatic ? "New messages: " : ""}{number(run.done)} of {number(run.planned)} · {number(run.found)} named · {spend(run.cost_usd, run.unknown_cost_requests)}</small>}
      {run.running && <span className="lane-progress" role="progressbar" aria-label="Ship finder progress" aria-valuemin={0} aria-valuemax={run.planned} aria-valuenow={run.done}><i style={{ width: `${run.planned ? run.done / run.planned * 100 : 0}%` }}/></span>}
      {stopping && <small role="status">Requests already sent finish and are recorded; no new one starts.</small>}
      {!run.running && run.stop_reason && <small className={run.stop_reason === "Stopped" ? undefined : "lane-error"}>{run.stop_reason === "Stopped" ? `Stopped after ${number(run.done)} of ${number(run.planned)}.` : run.stop_reason}</small>}
      {!run.running && run.finished_at && !run.stop_reason && run.planned > 0 && <small>Last run: {number(run.done)} messages, {number(run.found)} ships named, {spend(run.cost_usd, run.unknown_cost_requests)}.</small>}
      <div className="lane-actions">
        {run.running
          // aria-disabled, not disabled: the focused button keeps focus while it reads Stopping…
          ? <button className="processing-note" aria-disabled={act.isPending || stopping || undefined} onClick={() => { if (act.isPending || stopping) return; setStopSent(true); act.mutate("stop"); }}>{stopping ? "Stopping…" : "Stop"}</button>
          : d.waiting > 0 && <button className="processing-note action" aria-describedby={noteId} disabled={act.isPending || paused || !d.evaluated} title={!d.evaluated ? unevaluated : paused ? "Processing is paused; resume it to find ships" : undefined} onClick={() => act.mutate("start")}>{act.isPending ? "Starting…" : `Find ships in ${number(d.waiting)} waiting ${d.waiting === 1 ? "message" : "messages"}`}</button>}
        {/* Switching off is always possible; switching on needs the evaluation. */}
        <label className="lane-switch" title={!d.evaluated && !autoOn ? unevaluated : "Checked every two minutes. One model request for each new message; KeelWatch matching is free."}><input type="checkbox" checked={autoOn} disabled={auto.isPending || (!d.evaluated && !autoOn)} onChange={e => auto.mutate(e.target.checked)}/>Find ships in new messages automatically</label>
      </div>
      {!run.running && d.waiting > 0 && d.evaluated && <small id={noteId}>Sends one model request for each waiting message, older ones included, to {modelName(d.model)}. KeelWatch matching is free.</small>}
      <small className={autoPaused ? "lane-error" : undefined}>{autoText}</small>
      {autoPaused && <button className="processing-note action" disabled={auto.isPending || !d.evaluated} title={d.evaluated ? undefined : unevaluated} onClick={() => auto.mutate(true)}>{auto.isPending ? "Resuming…" : "Resume automatic mode"}</button>}
      <ErrorMessage error={act.error ?? auto.error}/>
    </div>
    <span className="lane-usage" title={d.totals.unknown_cost_requests ? unknownHint : undefined}>{spend(d.totals.known_cost_usd, d.totals.unknown_cost_requests, " total")}</span>
  </div>;
}

function LinkLane({ links }: { links: NonNullable<ProcessingStatus["relationships"]> }) {
  const client = useQueryClient();
  const toggle = useMutation({
    mutationFn: (enabled: boolean) => api("/processing/relationships", z.record(z.string(), z.unknown()), { method: "PUT", body: { enabled } }),
    onSettled: () => { void client.invalidateQueries({ queryKey: ["processing"] }); },
  });
  const stopped = links.enabled && links.stopped;
  const semantic = links.semantic;
  // One action for the lane's actual state; the owner's click is the authorization.
  const control = links.activation_required ? { label: "Activate new matching", on: true, hint: "Compares reports by meaning as well as by place. Paid pairs already compared are reused." }
    : !links.enabled ? { label: "Turn on", on: true, hint: "Compare reports that may describe the same event" }
    : stopped ? { label: "Resume", on: true, hint: "Continue after reviewing the stop reason. Requests with unknown billing are not resent." }
    : { label: "Turn off", on: false, hint: "Stop comparing new reports; existing links stay" };
  return <div className="processing-lane branch" data-state={links.activation_required || stopped ? "attention" : !links.enabled ? "off" : links.running ? "live" : "on"}>
    <span className="lane-ring" aria-hidden/>
    <div>
      <strong>Event links</strong>
      <small>{number(links.links)} {links.links === 1 ? "link" : "links"} from {number(links.completed)} compared pairs{links.in_flight ? ` · ${number(links.in_flight)} comparing` : ""}</small>
      {semantic && semantic.vectors > 0 && <small>{number(semantic.vectors)} claims indexed by meaning · {number(semantic.candidates)} close pairs found</small>}
      {links.activation_required && <small className="lane-error">Matching changed; the new policy is not active yet. Existing links stay.</small>}
      {links.projection_errors > 0 && <small>{number(links.projection_errors)} readings could not be compared; originals are kept.</small>}
      {links.last_error && <small className="lane-error">{stopped ? "Stopped: " : ""}{links.last_error}</small>}
      <button className={`processing-note ${control.on ? "action" : ""}`} disabled={toggle.isPending} title={control.hint} onClick={() => toggle.mutate(control.on)}>{toggle.isPending ? "Saving…" : control.label}</button>
      <ErrorMessage error={toggle.error}/>
    </div>
    <span className="lane-usage" title={links.unknown_cost_requests ? unknownHint : undefined}>{spend(links.known_cost_usd, links.unknown_cost_requests, " total")}</span>
  </div>;
}

type Entry = { key: string; kind: string; status: string; at: string; accepted: number; total: number; batches: number; detail: string; job: Job };
const activeStates = new Set(["queued", "running", "cancelling"]);
const providers: Record<string, string> = { telegram_collection: "Telegram", x_collection: "X" };
/** Consecutive successful runs of one kind within three minutes read as one line. */
function entries(jobs: Job[]) {
  const out: Entry[] = [];
  for (const job of jobs) {
    // Page-level collection records and empty polls stay in the ledger only.
    if (job.kind === "collection" || (job.status === "completed" && job.accepted === 0)) continue;
    const last = out[out.length - 1];
    const quiet = job.status === "completed";
    if (last && quiet && last.status === "completed" && last.kind === job.kind && Date.parse(last.job.created_at) - Date.parse(job.created_at) <= 180_000) {
      last.accepted += job.accepted; last.total += job.total; last.batches += 1; last.job = job;
      continue;
    }
    out.push({ key: job.id, kind: job.kind, status: job.status, at: job.created_at, accepted: job.accepted, total: job.total, batches: 1, detail: job.detail, job });
  }
  return out;
}
function describe(e: Entry) {
  const n = number(e.accepted), messages = e.accepted === 1 ? "message" : "messages";
  const provider = providers[e.kind];
  if (e.status === "running" || e.status === "queued") return e.kind === "processing" ? `Reading ${number(e.total)} messages` : `${provider ?? label(e.kind)} running`;
  if (e.status === "cancelling") return "Stopping";
  if (e.status === "failed") return e.kind === "processing" ? "Reading failed" : `${provider ?? label(e.kind)} failed`;
  if (e.status === "partial") return e.kind === "processing" ? `Read ${n} of ${number(e.total)} messages` : `${provider ?? label(e.kind)} partly completed`;
  if (e.kind === "processing") return `Read ${n} ${messages}${e.batches > 1 ? ` · ${e.batches} batches` : ""}`;
  if (provider) return `${provider} · ${n} new ${messages}`;
  if (e.kind === "import") return `Imported ${n} ${messages}`;
  return `${label(e.kind)} · ${n}`;
}

function Activity({ jobs }: { jobs: Job[] }) {
  const client = useQueryClient();
  const [all, setAll] = useState(false);
  const cancel = useMutation({
    mutationFn: (id: string) => api(`/jobs/${encodeURIComponent(id)}/cancel`, jobSchema, { method: "POST" }),
    onSuccess: () => { void client.invalidateQueries({ queryKey: ["workspace"] }); void client.invalidateQueries({ queryKey: ["processing"] }); },
  });
  const list = entries(jobs);
  const shown = all ? list : list.slice(0, 8);
  return <section className="processing-activity" aria-label="Recent activity">
    <h3>Activity</h3>
    <ErrorMessage error={cancel.error}/>
    {!list.length && <p className="processing-quiet">Nothing new yet.</p>}
    <ol>{shown.map(e => <li key={e.key} data-status={e.status}>
      <time dateTime={e.at} title={`${full.format(new Date(e.at))} UTC`}>{when(e.at)}</time>
      <span className="activity-dot" aria-hidden/>
      <div>
        <strong>{describe(e)}</strong>
        {(e.status === "failed" || e.status === "partial") && <small>{e.detail}</small>}
      </div>
      {activeStates.has(e.status) && <button className="button small" disabled={cancel.isPending || e.status === "cancelling"} onClick={() => cancel.mutate(e.job.id)}>{e.status === "cancelling" ? "Stopping…" : "Stop"}</button>}
    </li>)}</ol>
    {!all && list.length > 8 && <button className="text-button" onClick={() => setAll(true)}>Show {list.length - 8} earlier</button>}
  </section>;
}
