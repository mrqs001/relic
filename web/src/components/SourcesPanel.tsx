import { useEffect, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Anchor, ArrowLeft, ArrowRight, Check, ChevronRight, Database, Plus, Radio, X } from "lucide-react";
import { api, date, processingSchema, subscriptionSchema, subscriptionsSchema } from "../api";
import type { Subscription, Workspace } from "../api";
import { useDraft } from "../state";
import { Busy, ErrorMessage } from "./Common";
import { ImportButton } from "./CollectionPages";
import { ConnectionPanel, useConnections } from "./ConnectionPanel";
import { processingQuery } from "./ProcessingPanel";
import "../sources-panel.css";

const cadence = (s: Subscription) => s.mode === "stream" ? "Channel updates" : `Every ${s.poll_interval_seconds! % 60 === 0 ? `${s.poll_interval_seconds! / 60} min` : `${s.poll_interval_seconds} sec`}`;
const sourceStatus: Record<Subscription["status"], string> = {
  paused: "Paused", not_connected: "Not connected", waiting: "Up to date", running: "Collecting",
  catching_up: "Catching up", needs_attention: "Needs attention", blocked: "Blocked",
};

const statusLabel = (source: Subscription) => source.status === "waiting" && !source.collection?.last_success_at ? "Scheduled" : sourceStatus[source.status];

export default function SourcesPanel({ workspace }: { workspace: Workspace }) {
  const client = useQueryClient();
  const [tab, setTab] = useState<"subscriptions" | "imported">("subscriptions");
  const [editing, setEditing] = useState<Subscription | "new" | null>(null);
  const [search, setSearch] = useState(""), [query, setQuery] = useState("");
  const [cursors, setCursors] = useState<(string | null)[]>([null]);
  const [saved, setSaved] = useState("");
  const [imported, setImported] = useState<string | null>(null);
  useEffect(() => { const timer = setTimeout(() => { setQuery(search.trim()); setCursors([null]); }, 250); return () => clearTimeout(timer); }, [search]);
  const cursor = cursors[cursors.length - 1];
  const sources = useQuery({
    queryKey: ["subscriptions", query, cursor],
    queryFn: ({ signal }) => api(`/subscriptions?limit=50&query=${encodeURIComponent(query)}${cursor ? `&cursor=${encodeURIComponent(cursor)}` : ""}`, subscriptionsSchema, { signal }),
  });
  const editingId = editing && editing !== "new" ? editing.id : null;
  const detail = useQuery({
    queryKey: ["subscriptions", "detail", editingId],
    queryFn: ({ signal }) => api(`/subscriptions/${encodeURIComponent(editingId!)}`, subscriptionSchema, { signal }),
    enabled: editingId !== null,
  });
  const editingSource = editing && editing !== "new" ? detail.data && detail.data.version >= editing.version ? detail.data : editing : undefined;
  const onSaved = (source: Subscription) => {
    client.setQueryData(["subscriptions", "detail", source.id], source);
    void client.invalidateQueries({ queryKey: ["subscriptions"] });
    void client.invalidateQueries({ queryKey: ["connections"] });
    setSaved(`${source.label} saved · ${statusLabel(source).toLowerCase()}`);
    setEditing(null);
  };
  return <div className="sources-panel">
    <div className="source-panel-top">
      <div className="source-tabs" role="tablist" aria-label="Source collections">
        <button role="tab" aria-selected={tab === "subscriptions"} onClick={() => { setTab("subscriptions"); setImported(null); }}>Channels & profiles{sources.data && <span>{sources.data.total}</span>}</button>
        <button role="tab" aria-selected={tab === "imported"} onClick={() => { setTab("imported"); setEditing(null); }}>All sources<span>{workspace.sources.length}</span></button>
      </div>
      {tab === "subscriptions" ? <button className="button primary" onClick={() => { setEditing("new"); setSaved(""); }}><Plus size={15}/>Add source</button> : <ImportButton onImported={() => { void client.invalidateQueries({ queryKey: ["workspace"] }); }}/>} 
    </div>
    {tab === "subscriptions" ? <>
      <ConnectionPanel/>
      <div className={`source-manager ${editing ? "is-editing" : ""}`}>
        <section className="subscription-list" aria-label="Configured channels and profiles">
          {(sources.data?.total || query) ? <input className="source-find" aria-label="Find a configured source" placeholder="Find a channel or profile" value={search} onChange={e => setSearch(e.target.value)}/> : null}
          <ErrorMessage error={sources.error}/>
          {sources.isPending && <Busy>Loading sources…</Busy>}
          {saved && <p className="source-saved" role="status"><Check size={14}/>{saved}</p>}
          {sources.data?.items.length ? <>
            <div className="subscription-columns" aria-hidden="true"><span>Source</span><span>Collection</span><span>Status</span></div>
            {sources.data.items.map(source => <button className={`subscription-row ${editing !== "new" && editing?.id === source.id ? "selected" : ""}`} key={source.id} onClick={() => { setEditing(source); setSaved(""); }}>
              <span className="subscription-identity"><span className={`provider-mark ${source.provider}`}>{source.provider === "telegram" ? "TG" : "𝕏"}</span><span><strong>{source.label}</strong><small>{source.target}</small></span></span>
              <span className="subscription-cadence">{cadence(source)}</span>
              <span className={`subscription-status ${source.status}`} title={source.status_detail}>{statusLabel(source)}</span><ChevronRight size={14}/>
            </button>)}
            <div className="subscription-pagination"><span>{(cursors.length - 1) * 50 + 1}–{(cursors.length - 1) * 50 + sources.data.items.length} of {sources.data.total}</span><button className="icon-button" aria-label="Previous sources" disabled={cursors.length === 1} onClick={() => setCursors(old => old.slice(0, -1))}><ArrowLeft size={15}/></button><button className="icon-button" aria-label="Next sources" disabled={!sources.data.next_cursor} onClick={() => setCursors(old => [...old, sources.data!.next_cursor])}><ArrowRight size={15}/></button></div>
          </> : sources.data && <div className="sources-empty"><Database size={25}/><h2>{query ? "No matching sources" : "Add the channels you follow"}</h2><p>{query ? "Try another name or handle." : "Telegram channel updates or X profiles checked on a schedule."}</p>{!query && <button className="button" onClick={() => setEditing("new")}><Plus size={14}/>Add source</button>}</div>}
        </section>
        {editing && <SubscriptionEditor key={editingId ?? "new"} source={editingSource} detailError={detail.error} onSaved={onSaved} onClose={() => setEditing(null)}/>}
      </div>
    </> : <RetainedSources workspace={workspace} selected={imported} onSelect={setImported}/>}
  </div>;
}

type Source = Workspace["sources"][number];
const sourceKind = (source: Source) => source.fictional ? "Fictional bundle" : source.provider === "telegram" ? "Telegram channel" : source.provider === "x" ? "X profile" : source.family === "institutional_advisory" ? "Official warnings" : "Imported records";
function SourceMark({ source }: { source: Source }) {
  return source.provider === "telegram" ? <span className="provider-mark telegram">TG</span> : source.provider === "x" ? <span className="provider-mark x">𝕏</span> : <span className="provider-mark">{source.family === "institutional_advisory" ? <Anchor size={15}/> : <Database size={16}/>}</span>;
}

/** Every retained source, with the owner's choice of which ones Relic reads. */
function RetainedSources({ workspace, selected, onSelect }: { workspace: Workspace; selected: string | null; onSelect: (id: string | null) => void }) {
  const processing = useQuery(processingQuery);
  const read = new Set(processing.data?.policy.eligible_source_ids ?? []);
  const source = workspace.sources.find(s => s.id === selected);
  return <div className={`source-manager ${source ? "is-editing" : ""}`}>
    <section className="subscription-list" aria-label="Retained evidence sources">
      {workspace.sources.length > 0 && <div className="subscription-columns retained-columns" aria-hidden="true"><span>Source</span><span>Reading</span><span/></div>}
      {workspace.sources.map(item => <button className={`imported-source-row ${item.id === selected ? "selected" : ""}`} key={item.id} onClick={() => onSelect(item.id)}>
        <SourceMark source={item}/><span><strong>{item.label}</strong><small>{sourceKind(item)}</small></span>
        <span className={`source-reading ${read.has(item.id) ? "on" : ""}`}>{processing.data ? read.has(item.id) ? "Read" : "Not read" : ""}</span><ChevronRight size={14}/>
      </button>)}
      {!workspace.sources.length && <div className="sources-empty"><h2>No sources yet</h2><p>Add a channel or import a source bundle to retain and inspect its reports.</p></div>}
    </section>
    {source && <aside className="subscription-editor imported-detail">
      <div className="source-editor-heading"><h2>{source.label}</h2><button className="icon-button" aria-label="Close source details" onClick={() => onSelect(null)}><X size={16}/></button></div>
      {processing.data && !source.fictional && <SourceReading source={source} reading={read.has(source.id)} paused={!processing.data.policy.enabled} configured={processing.data.configured}/>}
      <dl><dt>Coverage</dt><dd>{source.coverage}</dd><dt>Availability</dt><dd>{source.detail}</dd></dl>
    </aside>}
  </div>;
}

function SourceReading({ source, reading, paused, configured }: { source: Source; reading: boolean; paused: boolean; configured: boolean }) {
  const client = useQueryClient();
  const change = useMutation({
    mutationFn: (next: boolean) => api(`/processing/sources/${encodeURIComponent(source.id)}`, processingSchema, { method: "PUT", body: { reading: next } }),
    onSuccess: value => {
      client.setQueryData(["processing"], value);
      for (const key of ["workspace", "evidence-query"]) void client.invalidateQueries({ queryKey: [key] });
    },
    onError: () => { void client.invalidateQueries({ queryKey: ["processing"] }); },
  });
  const on = change.isPending ? change.variables : reading;
  const advisory = source.family === "institutional_advisory";
  return <section className="source-reading-choice" aria-label="Reading">
    <label className="subscription-enabled"><input type="checkbox" checked={on} disabled={change.isPending || !configured} onChange={e => change.mutate(e.target.checked)}/>Read messages from this source</label>
    <p className="source-editor-note">{!configured ? "Reading is not configured in this workspace." : on
      ? `Relic sends each message's text to OpenRouter to find events, places and a translation.${paused ? " Reading is paused in Processing." : ""}`
      : "Messages stay in All messages, without events or places from reading."}</p>
    {advisory && <p className="source-editor-note">Warnings already appear on the map as their own layer, at the position the issuer gives. Reading also puts them in Event reports and lets them link with other reports of the same incident.</p>}
    <ErrorMessage error={change.error}/>
  </section>;
}

type SourceDraft = { provider: "telegram" | "x"; target: string; label: string; intervalMinutes: string; enabled: boolean; expected_version?: number };
function SubscriptionEditor({ source, detailError, onSaved, onClose }: { source?: Subscription; detailError: unknown; onSaved: (source: Subscription) => void; onClose: () => void }) {
  const client = useQueryClient();
  const connections = useConnections();
  const draft = useDraft<SourceDraft>(`relic:subscription:${source?.id ?? "new"}`, { provider: source?.provider ?? "telegram", target: source?.target ?? "", label: source?.label ?? "", intervalMinutes: String((source?.poll_interval_seconds ?? 120) / 60), enabled: source?.enabled ?? false, expected_version: source?.version });
  const value = draft.value;
  const set = (patch: Partial<SourceDraft>) => draft.setValue({ ...value, ...patch });
  const save = useMutation({
    mutationFn: (submitted: SourceDraft) => api(source ? `/subscriptions/${encodeURIComponent(source.id)}` : "/subscriptions", subscriptionSchema, {
      method: source ? "PUT" : "POST",
      body: { provider: submitted.provider, label: submitted.label.trim() || submitted.target.trim(), target: submitted.target, enabled: submitted.enabled, mode: submitted.provider === "telegram" ? "stream" : "poll", poll_interval_seconds: submitted.provider === "telegram" ? null : Math.round(Number(submitted.intervalMinutes) * 60), ...(source ? { expected_version: submitted.expected_version } : {}) },
    }),
    onSuccess: () => { void client.invalidateQueries({ queryKey: ["subscriptions"] }); },
    onError: () => { void client.invalidateQueries({ queryKey: ["subscriptions"] }); },
  });
  return <aside className="subscription-editor">
    <div className="source-editor-heading"><h2>{source ? "Edit source" : "New source"}</h2><button className="icon-button" aria-label="Close source editor" onClick={onClose}><X size={16}/></button></div>
    {source && <CollectionHealth source={source} allowed={connections.data?.collection_allowed === true} connected={connections.data?.providers.some(provider => provider.provider === source.provider && provider.status === "connected") === true}/>}
    <form onSubmit={e => { e.preventDefault(); save.mutate({ ...value }, { onSuccess: result => { draft.clear(); onSaved(result); } }); }}>
      <fieldset className="source-editor-fields" disabled={save.isPending} aria-label="Source settings">
      <div className="provider-choice" role="group" aria-label="Source provider"><button type="button" aria-pressed={value.provider === "telegram"} onClick={() => set({ provider: "telegram" })}>Telegram</button><button type="button" aria-pressed={value.provider === "x"} onClick={() => set({ provider: "x" })}>X</button></div>
      <label>{value.provider === "telegram" ? "Channel" : "Profile"}<input autoFocus required maxLength={200} value={value.target} onChange={e => set({ target: e.target.value })} placeholder={value.provider === "telegram" ? "@channel or https://t.me/channel" : "@profile or https://x.com/profile"}/></label>
      <label><span>Name <span className="optional">optional</span></span><input maxLength={120} value={value.label} onChange={e => set({ label: e.target.value })} placeholder="A name you recognize"/></label>
      {value.provider === "telegram" ? <div className="collection-mode"><Radio size={16}/><span><strong>Channel updates</strong><small>Receive messages and edits as they arrive.</small></span></div> : <label>Check every<div className="cadence-input"><input aria-label="Polling interval in minutes" type="number" required min={0.5} max={1440} step="any" value={value.intervalMinutes} onChange={e => set({ intervalMinutes: e.target.value })}/><span>minutes</span></div></label>}
      <label className="subscription-enabled"><input type="checkbox" checked={value.enabled} onChange={e => set({ enabled: e.target.checked })}/>Collect when connected</label>
      <p className="source-editor-note">{value.enabled ? "Collection starts when this account is connected. Turn this off to pause." : "Paused sources keep their retained messages."}</p>
      {source && <small className="source-updated">Last saved {date(source.updated_at, true)}</small>}
      {draft.recovered && <small className="source-updated">Local edit recovered.</small>}
      {draft.storageError && <p className="error-message">Draft recovery is unavailable. Save before closing.</p>}
      <ErrorMessage error={detailError}/>
      <ErrorMessage error={save.error}/>
      {source && source.version !== value.expected_version && <div className="source-version-conflict"><p>Saved settings changed: {source.label} ({source.target}), {cadence(source)}, {statusLabel(source).toLowerCase()}. Your local edits are still here.</p><button type="button" className="button" onClick={() => { set({ expected_version: source.version }); save.reset(); }}>Keep edits on current version</button></div>}
      <button className="button primary" disabled={save.isPending || !value.target.trim()}>{save.isPending ? "Saving…" : "Save source"}</button>
      </fieldset>
    </form>
  </aside>;
}

function CollectionHealth({ source, allowed, connected }: { source: Subscription; allowed: boolean; connected: boolean }) {
  const client = useQueryClient();
  const collect = useMutation({
    mutationFn: (action: "poll" | "continue" | "baseline") => api(`/subscriptions/${encodeURIComponent(source.id)}/collect`, subscriptionSchema, { method: "POST", body: { action, expected_version: source.version } }),
    onSuccess: value => {
      client.setQueryData(["subscriptions", "detail", value.id], value);
      void client.invalidateQueries({ queryKey: ["subscriptions"] });
      void client.invalidateQueries({ queryKey: ["workspace"] });
    },
    onError: () => { void client.invalidateQueries({ queryKey: ["subscriptions"] }); },
  });
  const canContinue = source.collection?.can_continue === true;
  const busy = source.status === "running" || source.status === "catching_up";
  const current = collect.isPending ? "Requesting collection…" : source.status_detail;
  return <section className="subscription-health" aria-label="Collection health">
    <div className="subscription-health-heading"><span className={`subscription-status ${source.status}`}>{statusLabel(source)}</span>{source.collection && <span className="subscription-health-count">{source.collection.messages_acquired.toLocaleString("en-GB")} retained</span>}</div>
    {source.collection?.stalled && source.collection.last_success_at && <p className="subscription-stalled" role="alert">Nothing collected since {date(source.collection.last_success_at, true)}. Messages from this source are missing until it recovers.</p>}
    <p role="status">{source.collection?.delayed ? "Catch-up is delayed. Newest retained messages may not cover the current interval." : current}</p>
    {source.collection && <dl><dt>Last caught up</dt><dd>{source.collection.last_success_at ? date(source.collection.last_success_at, true) : "No successful collection"}</dd>{source.collection.next_due_at && <><dt>Next check</dt><dd>{date(source.collection.next_due_at, true)}</dd></>}</dl>}
    {source.collection?.gap_recorded && <small>An earlier incomplete interval was abandoned when recent history was restarted.</small>}
    <ErrorMessage error={collect.error}/>
    {allowed && source.collection?.can_rebaseline && <div><p>Restarting keeps retained messages and records the incomplete interval as a gap.</p><button className="button small" disabled={collect.isPending || source.status === "running"} onClick={() => collect.mutate("baseline")}>Restart recent history</button></div>}
    {allowed && connected && source.enabled && <button className="button small" disabled={collect.isPending || busy} onClick={() => collect.mutate(canContinue ? "continue" : "poll")}>{collect.isPending ? "Requesting…" : source.status === "catching_up" ? "Catching up automatically" : canContinue ? "Retry catch-up" : "Check now"}</button>}
  </section>;
}
