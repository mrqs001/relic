# Relic application core

**Status:** implementation details and historical analyst APIs are listed below; this is not a completed event-intelligence release.

This is the Rust application for the first local investigation slice. It starts empty; only explicitly activated accounts and enabled source policies can resume acquisition on startup. Build the React frontend separately, then this process serves its production files and the versioned API on loopback.

Run these commands from the **Relic project root**:

```sh
npm --prefix web ci
npm --prefix web run build
cargo build --manifest-path app/Cargo.toml --locked --release
./app/target/release/relic-app --root . --data-dir ./data --port 8790
```

Open `http://127.0.0.1:8790`. Explicitly choose **Import fictional demonstration**, or upload a permitted JSON bundle following `examples/port-investigation.json`. Keep fictional and real evidence in separate runtime directories. The bundled example's sensor observations remain contextual originals; they are not published as reports or interpolated movement tracks.

For a disposable check, use a new empty `--data-dir` under `/tmp`. Nothing searches parent folders or another repository. `--root` locates `web/dist`; the supplied demonstration is compiled into the binary and remains portable. Runtime data paths default to the explicit configuration file's directory, or to `--root` without a configuration file.

Configuration loading is explicit:

```sh
./app/target/release/relic-app --root . --env .env
```

The process reads environment variables and only the exact file named by `--env`; environment variables take precedence. Supported application settings are `RELIC_HOST` (loopback only), `RELIC_PORT`, `RELIC_DATA_DIR` and `RELIC_SESSION_DIR`, plus documented private X/Telegram and OpenRouter/AI settings. CLI `--port` and `--data-dir` override configured values. `--env` alone does not activate a source or model; only an earlier saved account activation can reconnect automatically. Private values must not appear in commands, logs or browser build assets.

The owner-directed local application opens without a password. Legacy `RELIC_API_TOKEN` values are ignored. Host, Origin and cross-site checks remain active, and non-loopback binding is refused. The compatibility session endpoint reports local mode; the frontend has no unlock or lock control. Telegram/X provider authentication is separate.

## Implemented workflow

- Strict, bounded evidence import with original request bytes, normalized record JSON, exact Unicode text and immutable revisions. Semantically identical reimports reuse publication membership while preserving differing original byte representations as separate acquisitions.
- One Rust scope predicate for source, Unicode literal search, publication/acquisition time, dateline-aware bounds and unlocated inclusion. Map/list/counts share a pinned publication; pagination cursors cannot cross scope revisions. Source edit/publication time takes precedence over acquisition time when late older revisions arrive.
- Exact evidence inspection and original JSON download; supplied frozen baseline retains three reporting identities and report-1 v1, excluding the late authority statement.
- Investigations, exact evidence pins, notes, explicit claims, assessment history and optimistic concurrency conflicts. Assessed claims are immutable; changed meaning requires a new claim identity.
- Analyst-authored brief revisions with exact in-scope citations, explicit unsupported findings, reopen, Markdown export and structured provenance export. Exports distinguish per-record normalized JSON hashes from original input hashes.
- Watch preview without mutation, explicit fresh baseline, pause/resume, new-observed-origin and source-correction episodes. Copies attach to their origin episode without reopening a reviewed alert. A selected material change reopens review and retains its earlier review history. Automated contradiction detection is explicitly unavailable.
- Persisted executed lineage for acquisition, source revisions, processing, publication, assessments and brief citations. Bounded graph neighborhoods expose typed edges and an accessible frontend list. Hidden-node counts are explicitly lower bounds where graph limits apply.
- Import job ledger, cooperative cancellation before publication, interrupted-job recovery, consistent online backup and hash/citation-verified restore into a new directory. Ctrl-C stops admission, cancels running imports and closes SSE streams.

## Durability and bounds

SQLite 3.51.3 or newer is checked at runtime; the current lockfile links SQLite 3.53.2. Every connection enables foreign keys, `synchronous=FULL` and a 2-second busy timeout. WAL is enabled. SQL metadata and publication/event pointers commit atomically. Originals are content-addressed under `objects/sha256`; every new file is synced before rename, and all affected directories are synced before metadata advertises retention. Directory synchronization is batched per import to reduce filesystem overhead while preserving that order.

The database contains source/evidence identities, publications and exact membership, analyst work, watches/alerts, jobs, acquisitions, interpretations and typed lineage. FTS5 retains the lexical index; current query semantics are Unicode case-insensitive literal matching across original text and pinned translation. The raw-coordinate R-tree is retained; current scope predicates use `reading_evidence` coordinates so accepted derived city points share the same map/list/count scope. The 50k-record HTTP measurement is in `validation/PROCESSING.md`. Notes and brief revisions are independent of derived publications.

At most one import and four other blocking operations run concurrently; full capacity returns an explicit retryable 503 without an unbounded waiting queue. Each import accepts at most 2 MiB, 2,000 revisions, 100 source definitions and 64 KiB of text per revision. Map output is capped at 10,000 features, list pages at 500, lineage at 100 nodes, and SSE at 128 retained change events. Subset limits are disclosed. Cancellation is checked between file writes, directory syncs and transaction work; a commit already in progress can complete, and the returned job reports that outcome.

Exact membership materialization for brief exports and watch baselines/evaluation is capped at 100,000 records (`MAX_SCOPE_MEMBERS`, also exposed by health). The SQL query fetches at most the limit plus one and rejects the entire operation with HTTP 422 `scope_limit_exceeded` if exceeded; it never exports or saves truncated membership. Narrow the source, time or geographic scope to proceed. An active watch crossing this bound causes its candidate import to fail while retaining the previous complete publication; pause that watch before retrying the import. The ordinary paginated map/list query remains available.

A normal backup is an online SQLite snapshot plus its referenced immutable objects and checksums. The initial backup bound is 100,000 objects / 512 MiB. `/backup/{id}/verify` restores to a fresh `restore-verifications` directory, validates integrity, foreign keys, all object hashes, pins, assessments and every brief revision's exact support. It never replaces active data. To activate a verified restore, stop the current process and launch with that returned directory as `--data-dir`. Original-source, note and brief data should be backed up together; do not copy a live WAL database file alone.

This first schema adds tables idempotently but does not yet implement arbitrary schema migrations or automatic object pruning. Interrupted staging is discarded at startup; unreferenced immutable objects are retained pending an explicit reclamation policy. Publication history, analyst work and cited originals are not pruned automatically.

Remaining scale limits: workspace overviews and restore verification currently materialize analyst-record collections, historical watch-seen indexes are enumerated during evaluation, and backup metadata is enumerated before its object-count bound is checked. The SQLite snapshot file itself has no configured byte ceiling. Pagination or streamed enumeration and a snapshot-byte budget are required before treating those paths as bounded for an arbitrarily large archive. These limits are separate from the checked exact-membership export/watch limit.

## Verification and remaining boundaries

```sh
cargo test --manifest-path app/Cargo.toml --locked --all-targets
cargo fmt --manifest-path app/Cargo.toml --check
cargo clippy --manifest-path app/Cargo.toml --all-targets -- -D warnings
```

The application tests cover exact Unicode spans/search, frozen membership, revision immutability, dateline scopes/cursors, fictional separation, source identity, failed/cancelled publication retention, active cancellation, restart, backup corruption, query-scoped restore, claim immutability, alert review behavior, login/host/origin protection and original-upload byte round trips. The top-level `validation/workflow.mjs` exercises the complete HTTP workflow; production-browser and concurrent-load evidence are recorded separately by the main implementation task.

Flightradar24, KeelWatch, institutional advisory acquisition, broader AI recipes, recipe editing, incident merges/splits, arbitrary known-at-the-time replay and wider reference coverage remain later milestones. X and Telegram now have native account/collection paths and explicit controlled media loading; live Telegram verification still requires fresh user authorization. The supplied Rust integrations and their standalone protocol/proxy tests remain unchanged and authoritative. When source acquisition is connected, use that crate's proxy-only transport; no browser upstream asset path or direct fallback exists in this application. An offline pass or build is not a provider-availability, browser-performance or full first-release claim.


## X and Telegram collection

Open Sources to connect an account, then add public handles and enable their policies. Start live use with `./relic --env .env --data-dir data/live`; keep fictional fixtures in another directory. Before the first network connection the workspace is reserved for real account data, so a later fictional import cannot race with account activation. Each provider has one active operation; four bounded collection storage tasks are separate from four ordinary query workers. One publication writer provides backpressure. There is no unbounded network task queue.

Each attempt has three pages; initial acquisition is one recent page (20 requested records), not a backfill. X uses the supplied proxy-pinned session for the entire interval, ordinary monitored-author watermark overlap, retained continuations and explicit gaps. Telegram uses native grammers 0.10 with a private file session, fresh interactive authentication, acknowledged new/edit/delete delivery, two-minute history catch-up and a 20-ID reconciliation sweep. It never automatically joins channels. Up to64 delivered updates are processed per scheduler tick within a two-second budget; the native adapter's bounded128 queue stops on overflow and replays from durable state after reconnect. This is a bounded recovery policy, not proven high-volume throughput.

Every successful page syncs raw decoded provider output and exact normalized records before atomic SQLite publication/checkpoint advancement. Errors keep prior publications and acquired pages. A completed page is coherent retained evidence, not a claim of complete provider history. Processing's Stop remains stopped until explicitly resumed or policy edited; source disconnect and Telegram login cancellation interrupt active network waits. Account backoff survives restart.

Media is explicit, one download at a time, capped at8 MiB. X assets use only the custom proxy-only client; Telegram rechecks exact originating revision. Only validated JPEG/PNG/WebP/GIF/MP4 are retained and served locally. GET/media never initiates upstream traffic. No SVG, HTML or third-party embed is rendered. Application tests and `validation/check_evidence_media.mjs` guard the boundary; `cargo run --manifest-path app/Cargo.toml --example ui_media_fixture -- /tmp/new-relic-media-fixture` creates an offline fictional UI fixture in a new directory.

See `validation/COLLECTION_RUNTIME.md` for actual tests and remaining sustained/live limits. Changing the configured X account cannot silently reuse its prior account-bound interval; explicit account migration/rebaseline is not yet exposed. New source setup is public handles only; old-history backfill and private Telegram peer picking remain later work.

## Message processing

See `docs/CONFIGURATION.md` for the AI settings. The Rust worker uses `processing_ai` for strict translation/extraction, `processing_geo` for exact publisher place candidates, and `processing_store` for immutable interpretations and coherent publication. `reading_evidence` applies the same interpretation snapshot to map/list/counts, translation search, Signals, related reporting, watches and brief scope. Country labels and ambiguous, denied, planned or historical events do not become inferred occurrence points. Exact originals remain authoritative.

`cargo run --manifest-path app/Cargo.toml --example ui_processing_fixture -- /tmp/new-relic-reading-fixture` builds an isolated, explicitly fictional UI fixture from authored gold outputs without calling a model. This checks integration, not model quality. The separate `evaluate_processing` example makes at most eight explicitly configured model requests against fictional inputs, records actual results privately, and promotes an evaluation record only when all acceptance checks pass. Never run either fixture/evaluation Store opener against a running workspace.

`cargo run --manifest-path app/Cargo.toml --example evaluate_processing -- --replay /path/to/processing-evaluation` validates retained responses without network access or opening a workspace. It requires the matching fictional fixture hash, prompt, model and response identities; it creates `replay-summary.json` once. Rejected injection output is recorded separately from successful readings. See `validation/PROCESSING.md` before copying an evaluated gate into another workspace.
