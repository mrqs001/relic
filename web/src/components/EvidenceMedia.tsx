import { useEffect, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Download, File, Image, Music, Play } from "lucide-react";
import type { EvidenceDetail } from "../api";
import { mediaLabel, mediaPresentation } from "./evidence-media";
import type { MediaIdentity } from "./evidence-media";
import { downloadMedia, mediaBytes } from "./media-download";
import type { MediaProgress, MediaReceipt } from "./media-download";
import "../evidence-media.css";

export type MediaRecovery = { onOpenUpdatedMedia: (id: string, revision: string) => void; autoResolveMedia: boolean; onMediaRevision: (id: string, previous: string, revision: string) => void };
type Tile = { identity: MediaIdentity; reference: unknown; reason: string };
type Control = { load: () => Promise<void>; wait: () => void; idle: () => boolean };
const tileKey = (t: Tile) => `${t.identity.id}@${t.identity.revision}:${t.identity.index}`;
// Album parts are one post natively and carry no tag, nor does the main post
// when a part is open (the line above the gallery links it). Replies and
// inferred follow-ups keep a small tag that opens their exact original.
const tags: Record<string, { text: string; title: string }> = {
  reply: { text: "Reply", title: "Posted as a reply to this message. Open the original." },
  nearby: { text: "Follow-up", title: "Posted right after this message without its own text; inferred, not verified. Open the original." },
};

export default function EvidenceMedia({ evidence, onSelect, ...recovery }: { evidence: EvidenceDetail; onSelect: (id: string, revision: string) => void } & MediaRecovery) {
  const client = useQueryClient();
  // The server transfers one media file at a time and rejects overlap, so every
  // request, single or "Load images", waits its turn in one queue.
  const controls = useRef(new Map<string, Control>());
  const queue = useRef<Promise<void>>(Promise.resolve());
  const register = (key: string, control: Control | null) => { if (control) controls.current.set(key, control); else controls.current.delete(key); };
  const enqueue = (key: string) => {
    const control = controls.current.get(key);
    if (!control?.idle()) return;
    control.wait();
    queue.current = queue.current.then(() => controls.current.get(key)?.load()).catch(() => undefined);
  };
  const groups = [{ id: evidence.id, revision: evidence.revision, reason: "self", media: evidence.media ?? [] }, ...(evidence.media_context?.members ?? [])];
  const tiles: Tile[] = groups.flatMap(g => g.media.map((reference, index) => ({ identity: { id: g.id, revision: g.revision, index }, reference, reason: g.reason })));
  const anchor = evidence.media_context?.anchor;
  if (!tiles.length && !anchor && !evidence.media_context?.limited) return null;
  const images = tiles.filter(t => mediaLabel(t.reference) === "Image").length;
  const revisionChanged = (id: string, previous: string, revision: string) => {
    if (id === evidence.id) recovery.onMediaRevision(id, previous, revision);
    else { void client.invalidateQueries({ queryKey: ["evidence"] }); void client.invalidateQueries({ queryKey: ["evidence-query"] }); }
  };
  return <section className="evidence-media" aria-label="Message media">
    {anchor && <p className="media-anchor">{anchor.reason === "album" ? "Part of an album." : anchor.reason === "nearby" ? "Posted right after another message." : "Belongs to another message."} <button className="text-button" onClick={() => onSelect(anchor.id, anchor.revision)}>Open the main post</button></p>}
    {tiles.length > 0 && <div className="media-heading">
      <h3>Media <span>{tiles.length}</span></h3>
      {images > 1 && <button className="text-button" onClick={() => tiles.filter(t => mediaLabel(t.reference) === "Image").forEach(t => enqueue(tileKey(t)))}>Load {images === tiles.length ? "all" : `${images} images`}</button>}
    </div>}
    <div className="media-grid" data-count={Math.min(tiles.length, 3)}>
      {tiles.map((tile, i) => <MediaTile key={tileKey(tile)} {...tile} ordinal={i + 1} register={control => register(tileKey(tile), control)} onRequest={() => enqueue(tileKey(tile))}
        source={evidence.source_label} onOpen={() => onSelect(tile.identity.id, tile.identity.revision)} {...recovery} onMediaRevision={revisionChanged}/>)}
    </div>
    {evidence.media_context?.limited && <p className="evidence-media-note">More nearby messages are in the feed.</p>}
  </section>;
}

function MediaTile({ identity, reference, reason, ordinal, register, onRequest, source, onOpen, autoResolveMedia, onMediaRevision, onOpenUpdatedMedia }: Tile & { ordinal: number; register: (control: Control | null) => void; onRequest: () => void; source: string; onOpen: () => void } & MediaRecovery) {
  const client = useQueryClient();
  const [displayError, setDisplayError] = useState<string | null>(null);
  const [error, setError] = useState<Error | null>(null);
  const [progress, setProgress] = useState<MediaProgress | null>(null);
  const [resolved, setResolved] = useState<MediaReceipt | null>(null);
  const [wide, setWide] = useState(false);
  const [waiting, setWaiting] = useState(false);
  const active = useRef<AbortController | null>(null);
  useEffect(() => {
    const cancel = () => active.current?.abort();
    window.addEventListener("pagehide", cancel);
    return () => { cancel(); active.current = null; window.removeEventListener("pagehide", cancel); register(null); };
  }, []);
  const kind = mediaLabel(reference);
  const name = `${kind} ${ordinal}`;
  const cached = useQuery<MediaReceipt>({ queryKey: ["media", identity.id, identity.revision, identity.index], enabled: false });
  const data = cached.data;
  const updated = resolved && resolved.revision !== identity.revision ? resolved : null;
  const stop = () => { active.current?.abort(); active.current = null; setProgress(null); };
  const load = async () => {
    setWaiting(false);
    if (active.current || data) return;
    const controller = new AbortController();
    active.current = controller;
    setDisplayError(null); setError(null);
    setProgress({ kind: "progress", phase: "connecting", downloaded: 0, total: null });
    try {
      const receipt = await downloadMedia(identity, controller.signal, value => {
        if (active.current === controller && !controller.signal.aborted) setProgress(value);
      });
      if (active.current !== controller || controller.signal.aborted) return;
      setResolved(receipt);
      client.setQueryData(["media", identity.id, receipt.revision, identity.index], receipt);
      if (receipt.revision !== identity.revision && autoResolveMedia)
        onMediaRevision(identity.id, identity.revision, receipt.revision);
    } catch (cause) {
      if (active.current !== controller) return;
      if (!controller.signal.aborted) setError(cause instanceof Error ? cause : new Error("Media could not be loaded."));
    } finally {
      if (active.current === controller) { active.current = null; setProgress(null); }
    }
  };
  useEffect(() => register({ load, wait: () => setWaiting(true), idle: () => !data && !active.current && !waiting && !updated }));
  const presentation = data ? mediaPresentation(data.media_type) : null;
  const Icon = kind === "Image" ? Image : kind === "Video" ? Play : kind === "Audio" ? Music : File;
  const percent = progress?.phase === "downloading" && progress.total ? Math.min(100, 100 * progress.downloaded / progress.total) : undefined;
  const progressText = !progress ? "" : progress.phase === "connecting" ? "Connecting…" : progress.phase === "verifying" ? "Preparing…" : progress.total ? `${mediaBytes(progress.downloaded)} of ${mediaBytes(progress.total)}` : progress.downloaded ? mediaBytes(progress.downloaded) : "Downloading…";
  const tag = tags[reason];
  const shown = data && !displayError && presentation !== "download";
  return <figure className="media-tile" data-state={shown ? "shown" : progress || waiting ? "loading" : error ? "error" : "idle"} data-kind={kind} data-wide={wide || (shown && presentation !== "image") ? "" : undefined}>
    {shown && presentation === "image" && <button className="media-view" aria-label={wide ? `Shrink ${name.toLowerCase()}` : `Enlarge ${name.toLowerCase()}`} onClick={() => setWide(!wide)}>
      <img src={data.url} alt={`${source} · ${name.toLowerCase()}`} onError={() => setDisplayError("This image could not be displayed.")}/>
    </button>}
    {shown && presentation === "video" && <video src={data.url} controls autoPlay preload="metadata" aria-label={`${source} · ${name.toLowerCase()}`} onError={() => setDisplayError("This video could not be played here.")}/>}
    {shown && presentation === "audio" && <audio src={data.url} controls preload="metadata" aria-label={`${source} · ${name.toLowerCase()}`} onError={() => setDisplayError("This audio could not be played here.")}/>}
    {!shown && <button className="media-placeholder" disabled={!!progress || waiting || (!!data && presentation === "download")} onClick={() => updated ? onOpenUpdatedMedia(identity.id, updated.revision) : onRequest()} aria-label={updated ? "View the updated message" : `Load ${name.toLowerCase()}`}>
      <Icon size={18} aria-hidden/>
      <span>{progress ? progressText : waiting ? "Queued" : updated ? "Updated message ↗" : displayError ? displayError : error ? "Try again" : data ? "File" : kind === "Video" ? "Play video" : `Load ${kind.toLowerCase()}`}</span>
      {data && <small>{mediaBytes(data.bytes)}</small>}
    </button>}
    {progress && <><progress aria-label={`Loading ${name.toLowerCase()}`} aria-valuetext={progressText} max={100} value={percent}/><button className="media-stop" onClick={stop} aria-label={`Stop loading ${name.toLowerCase()}`}>Stop</button></>}
    {tag && <button className="media-tag" title={tag.title} onClick={onOpen}>{tag.text} ↗</button>}
    {data && <a className="media-save" href={data.url} download aria-label={`Save ${name.toLowerCase()}`} title={`Save · ${mediaBytes(data.bytes)}`}><Download size={13}/></a>}
    {error && <figcaption className="media-error" role="status">{error.message}</figcaption>}
  </figure>;
}
