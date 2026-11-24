import { MessageTime } from "./MessageTime";
import { useEffect, useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { ArrowLeft, MoreHorizontal } from "lucide-react";
import { api, evidenceDetailSchema } from "../api";
import type { Citation, EvidenceDetail, Scope } from "../api";
import { Busy, ErrorMessage } from "./Common";
import EvidenceReading from "./EvidenceReading";
import type { MediaRecovery } from "./EvidenceMedia";
import "../reading.css";
import MessageLineage from "./MessageLineage";
import EventTimeline from "./EventTimeline";
import VesselFinder from "./VesselFinder";

type Props = MediaRecovery & {
  id: string | null; revision: string | null; snapshot?: string | null; scope: Scope;
  related?: { key: string; count: number } | null;
  onRelated: (key: string) => void;
  onLocate: (coordinates: [number, number] | [number, number, number, number]) => void;
  onSelect: (id: string, revision: string) => void;
  onMessageKey: (id: string, revision: string, key: string) => void;
};
export default function EvidenceInspector(props: Props) {
  const { id, revision, snapshot } = props;
  const detail = useQuery<EvidenceDetail>({
    queryKey: ["evidence", id, revision, snapshot ?? null], enabled: !!id && !!revision,
    // A publication changes background interpretation, not the selected source
    // revision. Preserve the reader/menu/player while refreshing that revision.
    placeholderData: previous => previous?.id === id && previous.revision === revision ? previous : undefined,
    queryFn: async ({ signal }) => {
      const data = await api(`/evidence?id=${encodeURIComponent(id!)}&revision=${encodeURIComponent(revision!)}${snapshot ? `&publication_id=${encodeURIComponent(snapshot)}` : ""}`, evidenceDetailSchema, { signal });
      if (snapshot && data.interpretation && data.interpretation_publication_id !== snapshot)
        throw new Error("This reading does not belong to the selected snapshot.");
      return data;
    },
  });
  useEffect(() => {
    if (detail.data?.message_key) props.onMessageKey(detail.data.id, detail.data.revision, detail.data.message_key);
  }, [detail.data]);
  useEffect(() => { if (detail.data) performance.mark("relic:evidence-visible"); }, [detail.data]);
  return <aside className="inspector slim-message" data-testid="evidence-inspector" data-evidence={detail.data?.id} data-revision={detail.data?.revision}>
    <ErrorMessage error={detail.error}/>{detail.isPlaceholderData && <small role="status" className="reading-pending">Updating reading…</small>}{detail.isLoading && <Busy/>}
    {detail.data && <Message key={`${id}@${revision}`} {...props} data={detail.data}/>}
  </aside>;
}
function Message({ data, scope, onRelated, onLocate, onSelect, autoResolveMedia, onMediaRevision, onOpenUpdatedMedia }: Props & { data: EvidenceDetail }) {
  const [view, setView] = useState<"reading" | "original" | "lineage">("reading");
  const [passage, setPassage] = useState<Citation | null>(null);
  const menu = useRef<HTMLDetailsElement>(null);
  const original = useRef<HTMLParagraphElement>(null);
  const source = data.source_label.replace(/^https?:\/\/t\.me\//, "@").replace(/^Fictional /, "");
  const platform = data.source.startsWith("telegram:") ? "Telegram" : data.source.startsWith("x:") ? "X" : "Source";
  const changeView = (next: typeof view) => { setView(next); if (menu.current) menu.current.open = false; };
  useEffect(() => {
    const outside = (event: PointerEvent) => { if (menu.current?.open && !menu.current.contains(event.target as Node)) menu.current.open = false; };
    document.addEventListener("pointerdown", outside);
    return () => document.removeEventListener("pointerdown", outside);
  }, []);
  useEffect(() => { if (view === "original" && passage) original.current?.querySelector("mark")?.scrollIntoView({ block: "center" }); }, [view, passage]);
  const chars = Array.from(data.text);
  return <>
    <header className="message-header">
      <span className="source-mark">{source.replace(/^@/, "").slice(0,1)}</span>
      <div className="message-byline"><h2 dir="auto">{source}</h2><span>{platform} · <MessageTime {...data}/></span></div>
      <details ref={menu} className="message-menu" onKeyDown={event => { if(event.key === "Escape" && menu.current?.open) {event.stopPropagation();menu.current.open=false;menu.current.querySelector("summary")?.focus();} }}><summary aria-label="Message options"><MoreHorizontal size={18}/></summary><div>
        <button onClick={() => { setPassage(null); changeView(view === "original" ? "reading" : "original"); }}>{view === "original" ? "Back to message" : "Original"}</button>
        <button onClick={() => changeView(view === "lineage" ? "reading" : "lineage")}>{view === "lineage" ? "Back to message" : "Lineage"}</button>
      </div></details>
    </header>
    <div className="inspector-scroll message-body">
      {data.fictional && <span className="fictional-label">Fictional</span>}
      {data.upstream_state === "removed" && <p className="message-state">Removed at source</p>}
      {view !== "reading" && <button className="message-back" aria-label="Back to message" onClick={() => changeView("reading")}><ArrowLeft size={13}/>{view === "original" ? "Original" : "Lineage"}</button>}
      {view === "reading" && <EvidenceReading evidence={data} onSelect={onSelect} related={null} onRelated={onRelated} onLocate={onLocate} autoResolveMedia={autoResolveMedia} onMediaRevision={onMediaRevision} onOpenUpdatedMedia={onOpenUpdatedMedia} onOriginal={() => { setPassage(null); changeView("original"); }}/>}
      {view === "reading" && <EventTimeline key={`${data.id}@${data.revision}`} id={data.id} revision={data.revision} scope={scope} onSelect={onSelect} onLocate={onLocate}/>}
      {view === "reading" && !data.fictional && <VesselFinder key={`vessels:${data.id}@${data.revision}`} id={data.id} revision={data.revision} publishedAt={data.published_at ?? data.acquired_at} onQuote={(start, end) => { setPassage({ evidence_id: data.id, revision: data.revision, start_character: start, end_character: end }); changeView("original"); }}/>}
      {view === "original" && <p ref={original} className="message-text original-text" dir="auto">{passage ? <>{chars.slice(0,passage.start_character).join("")}<mark>{chars.slice(passage.start_character,passage.end_character).join("")}</mark>{chars.slice(passage.end_character).join("")}</> : data.text || "No retained text"}</p>}
      {view === "lineage" && <MessageLineage evidence={data} onSelect={onSelect} onLocate={onLocate} onPassage={citation => { setPassage(citation); changeView("original"); }}/>}
    </div>
  </>;
}
