import {
  AlertCircle,
  ArrowUpRight,
  Check,
  LoaderCircle,
  X,
} from "lucide-react";
import type { ReactNode } from "react";
import { citationKey } from "../api";
import type { Citation } from "../api";
export function ErrorMessage({ error }: { error: unknown }) {
  return error ? (
    <div className="error-message" role="alert">
      <AlertCircle size={16} />
      <span>{error instanceof Error ? error.message : String(error)}</span>
    </div>
  ) : null;
}
export function Busy({
  children = "Loading retained records…",
}: {
  children?: ReactNode;
}) {
  return (
    <div className="loading" role="status">
      <LoaderCircle className="spin" size={17} />
      {children}
    </div>
  );
}
export function SaveState({
  pending,
  saved,
  dirty,
}: {
  pending: boolean;
  saved?: boolean;
  dirty?: boolean;
}) {
  return (
    <span
      className={`save-state ${saved && !dirty ? "success" : ""}`}
      role="status"
    >
      {pending ? (
        <>
          <LoaderCircle className="spin" size={12} /> Saving…
        </>
      ) : saved && !dirty ? (
        <>
          <Check size={12} /> Saved
        </>
      ) : dirty ? (
        "Draft · local recovery"
      ) : (
        ""
      )}
    </span>
  );
}
export function CitationList({
  citations,
  onOpen,
  onRemove,
}: {
  citations: Citation[];
  onOpen: (c: Citation) => void;
  onRemove?: (index: number) => void;
}) {
  return (
    <ol className="citation-list">
      {citations.map((c, i) => (
        <li key={`${citationKey(c)}:${i}`}>
          <button
            onClick={() => onOpen(c)}
            title={`Open exact citation ${citationKey(c)}`}
          >
            <b>[{i + 1}]</b>
            <span>
              <span className="mono">
                {c.evidence_id} · {c.revision}
              </span>
              <small>
                Characters {c.start_character}–{c.end_character}
              </small>
            </span>
            <ArrowUpRight size={14} />
          </button>
          {onRemove && (
            <button
              className="icon-button"
              onClick={() => onRemove(i)}
              aria-label={`Remove citation ${i + 1}`}
            >
              <X size={13} />
            </button>
          )}
        </li>
      ))}
    </ol>
  );
}
export function Empty({
  title,
  children,
  icon,
}: {
  title: string;
  children: ReactNode;
  icon?: ReactNode;
}) {
  return (
    <div className="empty-panel">
      {icon}
      <h3>{title}</h3>
      <p>{children}</p>
    </div>
  );
}
