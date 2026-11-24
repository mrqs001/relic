import { useRef, useState } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import {
  ArchiveRestore,
  ArrowUpRight,
  Database,
  FileText,
  Folder,
  GitBranch,
  HardDriveDownload,
  Import,
  LoaderCircle,
  Plus,
  ShieldCheck,
  Square,
  Upload,
} from "lucide-react";
import {
  api,
  backupSchema,
  date,
  importSchema,
  jobSchema,
  label,
  verifySchema,
} from "../api";
import type { Workspace } from "../api";
import { Empty, ErrorMessage } from "./Common";

export function ImportButton({
  onImported,
  className = "button",
  demo = false,
}: {
  onImported?: () => void;
  className?: string;
  demo?: boolean;
}) {
  const input = useRef<HTMLInputElement>(null),
    client = useQueryClient(),
    [fileError, setFileError] = useState("");
  const mutation = useMutation({
    mutationFn: (bundle?: unknown) =>
      api(demo ? "/imports/demo" : "/imports", importSchema, {
        method: "POST",
        body: demo ? undefined : bundle,
      }),
    onSuccess: () => {
      void client.invalidateQueries();
      onImported?.();
    },
  });
  return (
    <div className="import-control">
      <button
        className={className}
        disabled={mutation.isPending}
        onClick={() =>
          demo ? mutation.mutate(undefined) : input.current?.click()
        }
      >
        {mutation.isPending ? (
          <LoaderCircle className="spin" size={16} />
        ) : demo ? (
          <Database size={16} />
        ) : (
          <Upload size={15} />
        )}{" "}
        {mutation.isPending
          ? "Importing…"
          : demo
            ? "Load fictional demo"
            : "Import evidence bundle"}
      </button>
      {!demo && (
        <input
          ref={input}
          className="visually-hidden"
          type="file"
          accept=".json,application/json"
          aria-label="Import evidence JSON bundle"
          onChange={async (e) => {
            const file = e.target.files?.[0];
            if (!file) return;
            setFileError("");
            try {
              if (file.size > 10 * 1024 * 1024)
                throw new Error(
                  "This import is larger than the 10 MiB interactive limit. Split the bundle before importing.",
                );
              const data: unknown = JSON.parse(await file.text());
              mutation.mutate(data);
            } catch (error) {
              setFileError(
                error instanceof Error
                  ? error.message
                  : "The file could not be read.",
              );
            }
            e.target.value = "";
          }}
        />
      )}
      <ErrorMessage error={mutation.error || fileError} />
      {mutation.data && (
        <p className="helper" role="status">
          {mutation.data.accepted} accepted · {mutation.data.reused} already
          retained
          {mutation.data.warnings.length
            ? ` · ${mutation.data.warnings.join(" · ")}`
            : ""}
        </p>
      )}
    </div>
  );
}
export function LibraryPage({
  workspace,
  onInvestigation,
  onBrief,
  onNew,
  onLineage,
}: {
  workspace: Workspace;
  onInvestigation: (id: string) => void;
  onBrief: (id: string, investigation: string) => void;
  onNew: () => void;
  onLineage: (root: string) => void;
}) {
  const [filter, setFilter] = useState("");
  const backup = useMutation({
    mutationFn: () => api("/backup", backupSchema, { method: "POST" }),
  });
  const verify = useMutation({
    mutationFn: () =>
      api(
        `/backup/${encodeURIComponent(backup.data!.id)}/verify`,
        verifySchema,
        { method: "POST" },
      ),
  });
  const match = (v: string) =>
    v.toLocaleLowerCase().includes(filter.toLocaleLowerCase());
  return (
    <div className="destination-page library-page">
      <header className="page-header">
        <div>
          <span className="eyebrow">Retained work</span>
          <h1>Saved work</h1>
          <p>Notebooks, briefs and retained evidence.</p>
        </div>
        <ImportButton />
      </header>
      <div className="library-search">
        <input
          aria-label="Search saved work"
          placeholder="Search notebooks and briefs…"
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
        />
      </div>
      <div className="library-columns">
        <main>
          <section className="library-section">
            <div className="section-row">
              <h2>
                <Folder size={18} />
                Notebooks
              </h2>
              <button className="text-button" onClick={onNew}>
                <Plus size={13} />
                New notebook
              </button>
            </div>
            {workspace.investigations
              .filter((i) => match(i.title + i.question))
              .map((i) => (
                <button
                  className="library-row"
                  key={i.id}
                  onClick={() => onInvestigation(i.id)}
                >
                  <span className="library-row-icon">
                    <Folder size={18} />
                  </span>
                  <span>
                    <strong>{i.title}</strong>
                    <span>{i.question}</span>
                    <small>
                      {i.pins.length} pinned revisions · {i.assessments.length}{" "}
                      saved assessments
                    </small>
                  </span>
                  <ArrowUpRight size={16} />
                </button>
              ))}
          </section>
          <section className="library-section">
            <h2>
              <FileText size={18} />
              Saved briefs
            </h2>
            {workspace.briefs
              .filter((b) => match(b.title + b.finding))
              .map((b) => (
                <div className="library-row brief-row" key={b.id}>
                  <span className="library-row-icon">
                    <FileText size={18} />
                  </span>
                  <button
                    className="library-main-button"
                    onClick={() => onBrief(b.id, b.investigation_id)}
                  >
                    <strong>{b.title}</strong>
                    <span>{b.finding}</span>
                    <small>
                      {b.revision} ·{" "}
                      {b.authorship === "fixture"
                        ? "Fictional baseline"
                        : "Analyst authored"}{" "}
                      · cutoff {date(b.knowledge_cutoff, true)}
                    </small>
                  </button>
                  <button
                    className="icon-button"
                    aria-label={`Show lineage for ${b.title}`}
                    onClick={() => onLineage(`${b.id}@${b.revision}`)}
                  >
                    <GitBranch size={17} />
                  </button>
                </div>
              ))}
            {!workspace.briefs.length && (
              <p className="helper">
                Saved briefs appear here with their exact citations and frozen
                scope.
              </p>
            )}
          </section>
          <section className="library-section">
            <h2>
              <Database size={18} />
              Published evidence
            </h2>
            {workspace.publications_truncated && <p className="helper">Recent {workspace.publications.length} of {workspace.publications_total ?? "all"} publications. Saved briefs retain their exact snapshots.</p>}
            {workspace.publications.map((p) => (
              <div className="publication-row" key={p.id}>
                <div>
                  <strong>{p.label}</strong>
                  <span className="mono">{p.id}</span>
                  <small>
                    {date(p.created_at)} · {label(p.status)}
                  </small>
                </div>
                <span className="count">{p.count} records</span>
                <button className="text-button" onClick={() => onLineage(p.id)}>
                  Lineage ↗
                </button>
              </div>
            ))}
          </section>
        </main>
        <aside className="library-operations">
          <h2>Workspace recovery</h2>
          <p>
            A consistent database snapshot and its retained objects, with hashes
            and exact citations.
          </p>
          <button
            className="button full-width"
            disabled={backup.isPending}
            onClick={() => {
              verify.reset();
              backup.mutate();
            }}
          >
            <HardDriveDownload size={15} />
            {backup.isPending ? "Creating backup…" : "Create backup"}
          </button>
          <ErrorMessage error={backup.error} />
          {backup.data && (
            <div className="backup-result">
              <strong>Backup created</strong>
              <span>{date(backup.data.created_at)}</span>
              <span>{backup.data.objects} retained objects</span>
              <span className="mono break">{backup.data.id}</span>
              <p>{backup.data.detail}</p>
              <button
                className="button full-width"
                onClick={() => verify.mutate()}
                disabled={verify.isPending}
              >
                <ArchiveRestore size={14} />
                {verify.isPending
                  ? "Verifying restore…"
                  : "Verify restore in fresh directory"}
              </button>
            </div>
          )}
          <ErrorMessage error={verify.error} />
          {verify.data && (
            <div className="notice success" role="status">
              <ShieldCheck size={18} />
              <strong>
                {verify.data.verified
                  ? "Restore verified"
                  : "Restore verification failed"}
              </strong>
              <p>{verify.data.detail}</p>
              <span>
                {verify.data.objects} objects · {verify.data.briefs} briefs
              </span>
            </div>
          )}
          <div className="inspector-section">
            <h3>Import a permitted bundle</h3>
            <p className="helper">
              JSON imports preserve source identities, originals and immutable
              revisions. Failed candidates remain visible in Workbench.
            </p>
            <ImportButton />
            <ImportButton demo />
          </div>
          <div className="inspector-section">
            <h3>Reference geography</h3>
            <p className="helper">
              Natural Earth 1:110m, v5.1.2 supplies local locator geometry. Its
              scale is unsuitable for facility placement. The inherited
              collections remain unverified and unpublished.
            </p>
          </div>
        </aside>
      </div>
    </div>
  );
}
export function SourcesPage({ workspace }: { workspace: Workspace }) {
  return (
    <div className="destination-page">
      <header className="page-header">
        <div>
          <span className="eyebrow">Collection & coverage</span>
          <h1>Sources</h1>
          <p>
            Configured capability, actual retained coverage and remaining setup.
          </p>
        </div>
        <ImportButton />
      </header>
      <div className="notice source-notice">
        <ShieldCheck size={17} />
        <p>
          External collection is never started on page load. X and Flightradar24
          require their configured proxy for every upstream request, including
          media.
        </p>
      </div>
      <div className="source-table">
        <div className="source-table-head">
          <span>Source / evidence family</span>
          <span>Readiness</span>
          <span>Coverage & limitations</span>
        </div>
        {workspace.sources.map((s) => (
          <article className="source-table-row" key={s.id}>
            <div className="source-name">
              <span className="source-mark large">
                {s.label.replace("Fictional ", "")[0]}
              </span>
              <div>
                <h3>{s.label}</h3>
                <span>{label(s.family)}</span>
                <small>
                  {s.provider}
                  {s.fictional ? " · fictional" : ""}
                </small>
              </div>
            </div>
            <span
              className={`source-status ${s.status === "ready" || s.status === "available" ? "good" : ""}`}
            >
              <i />
              {label(s.status)}
            </span>
            <div>
              <strong>{s.coverage}</strong>
              <p>{s.detail}</p>
            </div>
          </article>
        ))}
      </div>
      {!workspace.sources.length && (
        <Empty
          title="No configured source metadata"
          icon={<Import size={28} />}
        >
          Import a permitted bundle to begin. Source availability is reported by
          the application core.
        </Empty>
      )}
      <section className="source-footer">
        <h3>Configuration remains private</h3>
        <p>
          Configure source credentials and proxy addresses in the explicit
          server runtime configuration. This interface receives capability
          metadata only. A configured key does not imply authenticated, entitled
          or complete coverage.
        </p>
      </section>
    </div>
  );
}
export function JobList({ workspace }: { workspace: Workspace }) {
  const client = useQueryClient();
  const cancel = useMutation({
    mutationFn: (id: string) =>
      api(`/jobs/${encodeURIComponent(id)}/cancel`, jobSchema, {
        method: "POST",
      }),
    onSuccess: () => {
      void client.invalidateQueries({ queryKey: ["workspace"] });
    },
  });
  return (
    <section className="jobs-panel">
      <div className="section-row">
        <h2>Processing runs</h2>
        <span className="helper">Last complete publication remains usable</span>
      </div>
      <ErrorMessage error={cancel.error} />
      {workspace.jobs.length ? (
        workspace.jobs.map((job) => (
          <div className="job-row" key={job.id}>
            <span className={`job-status ${job.status}`}>
              <i />
              {label(job.status)}
            </span>
            <div>
              <strong>{label(job.kind)}</strong>
              <p>{job.detail}</p>
              <small>
                {date(job.created_at, true)} · {job.accepted} / {job.total}{" "}
                accepted
              </small>
            </div>
            {["running", "queued"].includes(job.status) && (
              <button
                className="button small"
                disabled={cancel.isPending}
                onClick={() => cancel.mutate(job.id)}
              >
                <Square size={12} />
                Stop run
              </button>
            )}
            {job.status === "cancelling" && (
              <span className="helper">Stop requested…</span>
            )}
          </div>
        ))
      ) : (
        <p className="helper">No processing runs have been recorded.</p>
      )}
    </section>
  );
}
