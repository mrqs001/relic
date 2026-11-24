# Relic API v1 — first slice

## Message reading additions

Scope additionally accepts `content_view: "all" | "signals" | "events"` (API default `all`; the viewer opens on `events` since 10 October 2026) and `event_type: string | null` (default null) and `related_key: string | null` (default null; a returned SHA-256 related-reporting key). These fields apply through the single map/list/count/watch/brief predicate. Signals omits only processed promotion/other. Events contains messages with a reported event claim, excluding promotions (a subset of Signals; 25 September 2026). Search includes the retained original and its pinned translation. Related reporting is a shared city/topic/publication-day context, not an incident merge.

Evidence query items optionally include `interpretation`: ID, recipe/model/time, language, translation, headline, kind, claim status, topic, exact cited claims, attributed location candidates, optional approximate city geometry and a scope-filtered related count/key. The legacy `geometry` summary uses source coordinates when present, otherwise the publication’s accepted single derived point; it is not the map membership contract. Original stored evidence is unchanged. `GET /evidence?id=…&revision=…&publication_id=…` returns original geometry and the interpretation pinned by the requested publication; omitting publication uses the current one. Brief exports include the interpretation alongside exact raw support.

List order is newest first on the selected time basis; publication-time ordering uses acquisition time only when publication time is absent. Equal times use descending evidence row identity. Cursors remain bound to the exact scope and publication. Interpretation `support_alignment` records exact original quotations/scalar offsets and whether canonical-equivalence alignment was necessary; it does not replace or normalize the retained original.

| Method/path | Behavior |
| --- | --- |
| `GET /processing` | Redacted configuration/evaluation readiness, saved policy, processed/unread counts, actual `queued`/`in_flight`, `failed`, unavailable-input breakdown, nullable `quality_notice`, running state and actual/unknown usage accounting. |
| `PUT /processing/policy` | Persist `{enabled, eligible_source_ids}` plus a dated operator eligibility declaration in authoritative metadata when the selected source set changes. Exact retained source IDs only; no unknown fields. Collection/account activation remains separate. Returns status. |
| `POST /processing/run` | Admit one bounded batch of pending eligible revisions after evaluation and source-eligibility checks; returns running status immediately. Up to 256 messages per publication and 64 concurrent requests, with immediate continuation under an enabled policy. |
| `POST /processing/stop` | Cancel the current processing worker; returns status. The UI first disables the policy when pausing. |

These routes use the same loopback/Host/Origin boundary and 4 KiB mutation-body bound as source operations. The executable has no workspace password. The viewer has no manual brief/investigation entry points; existing analyst-work APIs and records remain backward compatible. Message options expose Original, Details and compact direct-connection Provenance.

No endpoint accepts arbitrary model instructions, provider keys or direct coordinate overrides.

This document is the shared Rust/TypeScript implementation boundary. Backend owns predicates and derived counts. JSON uses opaque string IDs, UTC RFC3339 timestamps and Unicode scalar offsets (start inclusive/end exclusive). All errors are HTTP 4xx/5xx `{error:{code,message}}`; never stringify private configuration/HTTP client errors. Base `/api/v1`. Query responses echo `generation` and stable `scope_revision`, and carry `publication_id`. Requests are bounded; unavailable data is an error or explicit coverage status, never a success-shaped empty fallback.

## Shared shapes

`Ref = {evidence_id:string, revision:string}`.

`Citation = Ref & {start_character:number,end_character:number}`.

`Evidence = {id, revision, source, source_label, family, text, published_at:string|null, acquired_at, source_edited_at?:string, event_at?:string|null, geometry:GeoJSON.Point|null, location_precision, origin_group:string|null, dependence?:string, copies_from?:{id,revision}, supersedes?:{id,revision}, language, fictional:boolean, availability, object_hash, citation:Citation|null, upstream_state?:string, media?:object[], relationships?:object}`. `object_hash` identifies normalized canonical record JSON, not the upload's original bytes. Evidence detail also returns `revisions:string[]`, `claims:Claim[]`, `original_input_hashes:string[]` and `lineage_root:string`. Original-input hashes identify the actual retained upload/file bytes. No source asset is fetched by the browser from an upstream URL.

`Scope = {source_ids:string[],include_unlocated:boolean,query:string,time_basis:'publication_time'|'acquisition_time',from:string|null,to:string|null,bbox:[west,south,east,north]|null,publication_id:string|null,content_view?:'all'|'signals'|'events',event_type?:string|null,related_key?:string|null}`. An empty source array means all sources; bounds inclusive, dateline supported. Latest complete publication selected if null. No viewport coupling until Search this view. One latest retained revision per identity in each publication; a frozen baseline preserves explicit membership. Source filtering happens before origin counts.

`Source = {id,label,family,provider,status,coverage,fictional:boolean,detail:string}`. Configuration is metadata only.

`Assessment = {id,claim_id,status:'supported'|'contested'|'insufficiently_supported'|'not_assessed',rationale,citations:Citation[],scope:Scope,publication_id,created_at,actor}`.

`Claim = {id,text,state:'asserted'|'denied'|'corrected'|'unresolved',citations:Citation[]}`.

`Investigation = {id,title,question,scope:Scope,version:number,notes:string,pins:(Ref & {reason:string})[],assessments:Assessment[],claims:Claim[]}`. Notes/durable versions independent of processing. Mutation conflict HTTP 409 if `expected_version` mismatches.

`Brief = {id,revision,title,investigation_id,question,finding,assessment,limitations:string[],citations:Citation[],scope:Scope,publication_id,knowledge_cutoff,created_at,authorship:'analyst'|'fixture',version:number}`. A finding without citations must be explicitly marked unsupported via `unsupported:true`. Existing brief saves add revisions; no automatic rewrites. Exports include exact quoted support, structured scope/membership, source provenance and omissions.

## Routes

| Route | Request / response |
| --- | --- |
| GET `/session` | Public session state `{authenticated:boolean,mode:'token'\|'local'}`. `local` explicitly means no configured workspace token and loopback-only operation. |
| POST `/session/login` | Public JSON `{token:string}` → session state plus an opaque session cookie. Token maximum 8,192 bytes; session-request body maximum 16 KiB. Wrong credentials return HTTP 401 `unauthorized`; malformed input returns structured HTTP 400. No token or cookie identifier appears in the JSON response. |
| POST `/session/logout` | Revokes this browser's session and clears its cookie → session state. Token mode returns `authenticated:false`; local mode remains `authenticated:true`. |
| GET `/health` | `{status,api_version:1,sqlite_version,limits:{...}}` |
| GET `/workspace` | `{name:'Relic',empty:boolean,fictional:boolean,sources:Source[],investigations:Investigation[],briefs:Brief[],publications:{id,label,created_at,count,status}[],watches:Watch[],jobs:Job[],coverage:string[],sequence:number}` |
| POST `/imports/demo` | No input. Explicitly imports bundled fixture idempotently; response `{publication_id,accepted,reused,warnings:string[],original_input_hash}`. No startup seed. |
| POST `/imports` | JSON evidence bundle matching supplied `examples/port-investigation.json` shape. Strictly bounded and validated. Return same summary; failed candidate remains inspectable. Preserve supplied non-execution lineage only as original input; create actual run lineage. |
| POST `/query` | `{scope:Scope,generation:number,cursor?:string,limit?:number}` → `{scope_revision,publication_id,generation,total,located,unlocated,origin_groups,unknown_origins,loaded,items:Evidence[],map:GeoJSON.FeatureCollection,map_loaded:number,map_truncated:boolean,next_cursor:string|null,coverage:string[]}`. Map bounded at 10k sites and discloses truncation. List limit 100/default, max 500. Map properties `{evidence_id,revision,source,title}`. |
| GET `/evidence?id=...&revision=...` | Evidence detail. Must resolve exact historical revision regardless of current scope. |
| GET `/objects/{hash}` | Download retained bytes by SHA-256. Uses the same workspace authentication and origin boundary; no upstream request. Semantically identical uploads may preserve distinct byte hashes without creating a new publication. |
| POST `/investigations` | `{title,question,scope}` → Investigation |
| PUT `/investigations/{id}` | `{expected_version,notes?,scope?,title?,question?,pins?,claims?}` → Investigation. Claim identities are unique; a claim with retained assessments cannot be rewritten or removed. Changed meaning requires a new claim identity. |
| POST `/investigations/{id}/assessments` | `{expected_version,claim_id,status,rationale,citations,scope,publication_id}` → Investigation |
| POST `/briefs` | `{id?:string,expected_version?:number,title,investigation_id,question,finding,assessment,limitations,citations,scope,publication_id,unsupported?:boolean}` → Brief |
| GET `/briefs/{id}?revision=...` | Brief exact revision; omit revision for latest. |
| GET `/briefs/{id}/export?revision=...&format=markdown\|json` | Download readable Markdown or structured manifest with exact retained support and scope. |
| GET `/lineage?root=...&depth=...&direction=both\|upstream\|downstream` | `{nodes:{id,kind,label,detail:object}[],edges:{id,source,target,relationship,detail:object}[],hidden_nodes:number,snapshot:string}`. Max 100 nodes; default root latest publication; evidence root `id@revision`, brief root `id@revision`. |
| GET `/lineage/message?root=id@revision` | One message revision's recorded path: `{root,nodes,edges,readings,hidden_nodes,hidden_count_is_lower_bound,snapshot,limits}` with the node/edge shape above. Includes the revision, its `superseded_by` version chain, every reading with a `supports` edge from it, and the direct connections of the revision and those readings (source, runs, reference editions, publications, supplementary and event-relationship records). A retaining run also contributes its `collected_by` source and `published` publication. Shared runs, publications and references are never expanded to their other outputs. `readings` summarizes each reading from its retained interpretation: `{id,evidence_id,revision,created_at,model,recipe,processing_mode,prior_interpretation_id,reference_edition,headline,language,kind,claim_status,location_label,location_precision,locations,claims,events}` (counts for the last four array fields), ordered by `created_at`. Bounds: 160 nodes, 200 edges per direction per node, 50 versions, 40 readings. Readings beyond the bound count toward `hidden_nodes`. A non-message root is `invalid_input`; an unknown root is `not_found`. |
| POST `/watches/preview` | `{title,question,scope,conditions:string[]}` → `{matched,groups,frequency:'unknown',coverage:string[],baseline_policy:'fresh',estimated_cost:'no external calls'}`; no mutation. |
| POST `/watches` | Same as preview plus `baseline_policy:'fresh'` → Watch; baseline taken now and persisted with rule. |
| PUT `/watches/{id}` | `{status?:'active'|'paused'}` → Watch |
| GET `/alerts` | `{items:Alert[]}` |
| PUT `/alerts/{id}` | `{review_status:'new'|'acknowledged'|'irrelevant'|'snoozed',snoozed_until?:string}` → Alert. |
| GET `/jobs` | `{items:Job[]}` |
| POST `/jobs/{id}/cancel` | Job with actual acknowledged cancellation state. |
| POST `/backup` | Creates consistent snapshot and object manifest inside runtime backup directory → `{id,created_at,objects,verified:boolean,detail}`. |
| POST `/backup/{id}/verify` | Restore to a fresh verification directory and verify FK/hashes/citations → `{verified,objects,briefs,detail}`. Never overwrite active data. |
| GET `/events` | SSE sequence/publication/affected IDs; bounded replay, `resync` on missed history. One stream per UI session. |

`Watch = {id,title,question,scope,conditions:string[],status,baseline_publication_id,created_at,frequency:'unknown'}`.

`Alert = {id,watch_id,reason,evidence:Ref[],origin_group:string|null,first_at,latest_at,review_status,delivery_status,snoozed_until?:string}`.

`Job = {id,kind,status,accepted:number,total:number,created_at,detail:string}`.

First-slice watch conditions are `new observed origin` and `source correction`. Contradiction detection remains unavailable until attributed claim-state evaluation is implemented; unsupported conditions are rejected. Manual comparison and contested assessments work independently. Empty-workspace queries return an explicit 404 until a complete publication exists.

Exact watch/brief membership is bounded at 100,000 records (`health.limits.exact_scope_members`). Overflow returns HTTP 422 `scope_limit_exceeded` with guidance to narrow scope or pause the oversized watch; it never returns truncated exact membership. An active watch that exceeds this bound rolls back the candidate import publication, preserving the last complete result. Query pagination/map limits remain separate.

## Workspace access

Owner-directed local mode (11 September 2026): the executable ignores legacy `RELIC_API_TOKEN` configuration and opens without workspace authentication. The frontend contains no unlock screen or lock action. Loopback binding, Host/Origin checks, cross-site rejection and private source-credential handling remain active. Telegram/X account authorization is separate.

`/session` remains a compatibility endpoint returning `{authenticated:true,mode:'local'}`. The old session machinery remains isolated and tested for an explicitly embedded token-mode router, but the shipped local configuration never enables it. No cookie or password is required for the local workspace routes described above.

## Integration process

Implementors may add optional fields and routes as useful, but coordinate changes here before dependent work. Add a machine-checkable response fixture or schema and test against it. Frontend runtime decoding rejects malformed core responses, supplies no manufactured records, and preserves acknowledged save versus draft state.

## Collection setup — saved Telegram and X subscriptions

`Subscription = {id:string,version:number,provider:'telegram'|'x',label:string,target:string,mode:'stream'|'poll',poll_interval_seconds:number|null,enabled:boolean,created_at:string,updated_at:string,status:'not_connected'|'paused'|'waiting'|'running'|'catching_up'|'needs_attention'|'blocked',status_detail:string,collection?:{last_success_at:string|null,next_due_at:string|null,messages_acquired:number,can_continue:boolean}}`.

| Route | Request / response |
| --- | --- |
| GET `/subscriptions?limit=50&cursor=...&query=...` | `{items:Subscription[],total:number,next_cursor:string|null}`. Creation order, keyset pagination. Page size 1–100; default 50. Search is a literal case-insensitive substring of provider, name or normalized target, at most 240 UTF-8 bytes. An opaque cursor is valid only for its search text. Each response is read from one SQLite snapshot; later pages reflect subsequently saved settings. |
| GET `/subscriptions/{id}` | Current Subscription by stable configuration ID, independently of list filters or pagination. Returns HTTP 404 if unavailable. Use this to refresh an editor after a version conflict. No acquisition side effects. |
| POST `/subscriptions` | `{provider,label,target,mode?,poll_interval_seconds?,enabled?}` → Subscription. X defaults to `poll` every 120 seconds; Telegram defaults to `stream` with a null interval. `enabled` defaults false. Unknown fields are rejected. |
| PUT `/subscriptions/{id}` | `{expected_version,provider,label,target,mode?,poll_interval_seconds?,enabled?}` → Subscription. Full replacement of editable fields, using the same defaults as creation. A missing/nonpositive version is invalid; a stale version returns HTTP 409. The configuration identity and creation time remain stable. |

Saved policy and account connection remain distinct. Enabling a connected source makes it due for bounded acquisition; otherwise it remains `not_connected`. Disabled sources remain `paused` except an explicit Check now/Continue operation. Saving a policy never itself advances a checkpoint. Configuration edits cancel stale work; handles resolve to stable provider identities before publication. The response status combines saved configuration, durable collection progress and current account state.

Names require 1–240 UTF-8 bytes after trimming, without control characters. Targets are bounded to 256 bytes before normalization. Accepted forms are a public handle, `@handle`, or an exact HTTPS public profile/channel URL: Telegram `t.me/handle` or `telegram.me/handle`; X `x.com/handle` or `twitter.com/handle` (optional `www.` for X). One trailing slash is accepted. Handles normalize to lowercase `@handle`. Telegram handles require 5–32 ASCII letters/digits/underscores and a leading letter; X handles require 1–15 ASCII letters/digits/underscores. Service routes, invites, private dialogs, post links, credentials, query strings, URL fragments, ports and other hosts are rejected. The API does not contact the provider to verify that a handle exists. Stable upstream duplicate detection remains a connection-stage requirement; at setup, duplicate provider + normalized handle returns HTTP 409.

Telegram supports only `stream` and a null interval. X supports only `poll`, with an integer interval of 30–86,400 seconds; omitted/null uses 120. This is desired cadence, not a freshness or delivery guarantee. There are at most 1,000 saved configurations; further creation returns HTTP 422 `subscription_limit`. Subscription request bodies are limited to 4 KiB and use the existing access/origin controls and bounded worker capacity.

SQLite transactions save settings and their `collection_settings` event together; SSE emits `{kind:'collection_settings',publication_id:null,affected_ids:[id],sequence}` only after commit. These settings do not create an evidence publication. The additive table is included in normal backup/restore. Restore verification checks saved configuration constraints and reports its `subscriptions` count; backups from before collection setup remain compatible. No credentials are stored in these settings.


## Account connections, collection and retained media

All routes below use the ordinary workspace access boundary. Source/auth/collection/media JSON bodies are capped at 4 KiB.

`Connection = {provider:'x'|'telegram',status:'unconfigured'|'disconnected'|'connecting'|'connected'|'awaiting_phone'|'awaiting_code'|'awaiting_password'|'error',detail:string,account_label:string|null,retry_at:string|null}`. Responses contain no credentials, private session contents, access hashes or proxy addresses.

`retry_at` is an account retry deadline, not proof of provider rate limiting: it may represent transport/reconnect backoff. The UI disables Connect with a countdown while it is in the future and refreshes status through expiry. Explicit Telegram flood waits and X HTTP 429 responses remain durable account gates; X 429 is handled during login, profile resolution, timeline and media operations. Existing untyped deadlines are honored until expiry, not silently removed.

A source lookup/access or X content/shape failure uses `source_unavailable`. Its failed job is retained and its subscription becomes `blocked`, with `next_due_at:null` and `last_error_code:"source_unavailable"` in collection state. It does not disconnect the account or set an account deadline. Other sources and controlled media remain usable. A matching explicit Check now/Continue or a configuration change releases the source hold; checkpoints, evidence and citations remain intact. Storage, authentication, transport and actual rate-limit failures retain their account/system handling. No missing-source response is treated as an empty successful interval.

An X GraphQL `errors` response is temporary rather than a source hold (27 September 2026): X returns it for passing timeline faults on readable profiles. It uses `source_transient`, keeps the account connected, marks only that source `needs_attention` with X's first error codes/messages in `detail`, and retries after 2, 5, 15, 30, then every 60 minutes (`transient_failures` counts consecutive failures; any published page resets it). Its catch-up continuation resumes under the same rules as a provider failure. The watermark still never advances on this response. Shutdown also cancels running collection, but that interruption is not a Stop: the source returns to `waiting` (or `catching_up` with its continuation) and is due on the next start. Only an operator Stop, saved before its run is cancelled, persists `stopped:true`. Revised 9 October: a failure or shutdown promises an automatic retry only when the scheduler will actually run it; a held interval (cursor safety limit, repeated cursor) says "It does not retry by itself: Continue the saved interval to try again." At the 60-minute step the detail also says for how long X has returned errors (`transient_since`, cleared by the next retained page). An operator Stop is remembered in memory, so a shutdown that races its save does not lose it.

| Route | Contract |
| --- | --- |
| GET `/connections` | `{providers:Connection[],collection_allowed:boolean,workspace_detail:string}`; fictional workspaces cannot connect. No provider call. |
| POST `/connections/{provider}/connect` | Empty object; explicit account access check → Connection. X checks the supplied custom GraphQL path through its proxy. Telegram resumes its private session or requests fresh phone authorization. |
| POST `/connections/{provider}/disconnect` | Empty object; stop account work, persist deactivation, retain evidence/session → Connection. |
| POST `/connections/telegram/auth` | Exactly one of `{phone:string}`, `{code:string}`, `{password:string}`, valid only in its matching connection state → Connection. Never retain these request values in browser queries/drafts, SQLite, logs or lineage. |
| POST `/subscriptions/{id}/collect` | `{action:'poll'|'continue',expected_version:number}` → Subscription; one bounded attempt. Poll cannot discard an unfinished interval. Rebaselining is rejected in this increment. |
| GET `/collection/metrics` | Current operation count, active/peak staged batch bytes, committed page count, uptime and explicit concurrency/page/media/queue limits. No source records or credentials. |
| POST `/media` | `{id:string,revision:string,index:number}` loads one exact retained reference through its connected adapter, validates/retains bytes, returns `{url:string,revision:string,media_type:string,bytes:number}`. `revision` is the resolved exact revision; normally it matches the request. Telegram can reconcile an upstream edit from its one exact-message lookup and return the newly retained revision. Its bytes belong only to that revision; the viewer switches message context before displaying them. Cached associations are reused. URL is always relative `/api/v1/media?...`. |
| POST `/media/download` | Same exact-reference body, response `application/x-ndjson`. The viewer uses this cancellable progress route. Frames are `{kind:"progress",phase:"connecting"\|"downloading"\|"verifying",downloaded:number,total:number\|null}`, followed by one `{kind:"ready",receipt:<same receipt as POST /media>}` or `{kind:"error",code:string,message:string}`. Once streaming starts, errors are terminal frames under HTTP 200; request/body/access failures retain HTTP error statuses. No automatic retry. |
| GET `/media?id=...&revision=...&index=...` | Retained binary stream with canonical Content-Type and nosniff; never fetches upstream. Supports HEAD and single byte ranges (206/416 and Content-Range). Read capacity is held until streaming ends or the client disconnects. Missing/corrupt assets return structured errors. |

Collections publish cumulative exact membership after every durable page, keeping raw decoded provider pages and normalized revisions separately. The X JSON decoder and sanitized Telegram TL representation define the retained original representation; no transport-byte fidelity is implied. Media-only messages keep empty exact text and `citation:null`; UI labels are presentation only. Removed/unavailable messages preserve retained history and carry `upstream_observed_at` without inventing a provider edit timestamp. Raw context from foreign/unresolved X authors is retained but not attributed to the monitored source.

Workspace publication summaries are bounded to the latest 100 (including current); `publications_total` and `publications_truncated` disclose the bound. Exact old publication and evidence IDs remain queryable. Live updates use connection/collection_state/collection/media_retained events or a resync signal; the client reloads coherent query state and never patches a guessed evidence position.

Telegram media reconciliation uses collection’s canonical timestamp/content fingerprint and existing atomic evidence/publication contract. It retains the sanitized exact-message response, preserves prior revisions/citations/publications, and leaves history watermarks, catch-up continuations and pause policy unchanged. Telegram uses the 512 MiB disk-streaming bound, 512 KiB transport chunks and ten-minute overall deadline; X retains its 8 MiB proxied transport bound. Media-type, provider-operation, rate-limit and source-configuration guards remain. A historical view offers the updated message explicitly. Failed recovery never substitutes newer bytes under an old revision.

The progress response owns its provider future: disconnect/abort drops that future and incomplete private staging, releasing transfer capacity. No detached download job is queued. A cancellation flag also reaches blocking file validation (checked per 64 KiB hash chunk and before durable commit). A fully validated asset already entering its short commit may remain cached; stopping does not erase completed immutable records. Progress uses a single latest-value slot, at most ten updates/second and a one-second heartbeat during stalled requests. Telegram totals come from the freshly fetched exact message, not a stale reference. X's unchanged bounded transport reports indeterminate progress until its response completes. 100% downloaded is not ready: only a validated receipt exposes playback. Legacy POST `/media` remains a completion-only compatibility operation, not the viewer's cancellation contract.

## Compact viewer URLs

The default map URL is `/`; message links use `/?m=<message_key>`. `message_key` is an additive string field on evidence list/detail responses and identifies that exact immutable SQLite evidence row within this workspace. `GET /messages/{key}` resolves `{id,revision}` under the same read/access limits. Unknown keys return 404; malformed keys return 400. SQLite backup/restore preserves these keys. A fresh import into a different workspace may assign different keys; portable exports and citations continue to use canonical provider ID plus full revision. Future retention must reserve expired keys and never reuse them for another message.

Only active filters appear as named parameters: repeated `source`, `q`, `located=1`, `time=received`, `range`, `from`, `to`, `bbox`, `snapshot`, `feed=all` (legacy `feed=events` and `feed=signals` remain readable), `type`, `related`. Sources uses `?view=sources`, Processing `?view=processing`. Legacy `scope=<JSON>`, `view=investigate/workbench` and full `evidence`/`revision` URLs remain readable and normalize after opening. Reload, copy/paste and browser Back/Forward resolve the same exact revision and scope; defaults and internal JSON stay out of ordinary URLs.

### Message timeline

Viewer `range=1h|2h|4h|8h|24h|1w` is a rolling duration, mutually exclusive with fixed `from/to`. It is frontend route state, not an API Scope field. Every visible 30-second clock tick resolves it into explicit inclusive UTC millisecond bounds in the normal `/query` request. Hidden tabs stop ticking and refresh on visibility. A relative link opens the latest window of that duration; fixed bounds preserve a historical interval. Pause holds bounds, not publication membership. Changing time never requests source backfill. Default `/` still means all retained time.

`POST /query` additionally returns `timeline: {from:string|null,to:string|null,time_basis,undated:number,buckets:{from:string,to:string,count:number}[]}`. At most 36 bins, computed on the exact shared predicate/publication before cursor pagination. Explicit bounds define the axis; otherwise dated matching extrema do. Empty unbounded results have null bounds and no bins. `sum(bucket.count) + undated === total`; publication-time windows exclude messages without publication timestamps, while All discloses their count. Received uses acquisition timestamps. No inferred occurrence timestamp or source-coverage claim is introduced. Bucket ranges are inclusive, non-overlapping millisecond intervals and can be replayed as exact Scope bounds. Historical publication, source, area, event-type and related filters apply identically. The UI preserves the previous coherent result while a new query generation is loading.


## Event reading contract — 12 September 2026

`message-events-v2` / `relic.message-events.v4` adds up to five typed claims. Vocabulary: `strike`, `armed_attack`, `explosion`, `air_defense`, `fire`, `protest`, `military_movement`, `infrastructure_disruption`, `accident`, `disaster`, `other`. Claims independently carry reported/denied/planned/historical/unclear state, nullable exact `time_text`, and zero-based `location_indices` into the existing location array. Indices must be unique, in range and supported by original mentions inside that claim's quote. Time phrases must occur inside the same quote; no event timestamp is inferred from publication time. Non-event attribute claims have null type/time and empty indices. New wire outputs must explicitly supply these keys; retained older DTOs deserialize with defaults.

Validated readings derive their summary status from the first substantive event claim. Coarse topics follow type: strike/armed attack/air defence/military movement → conflict; explosion/accident → safety; fire/disaster → disaster; protest → politics; disruption → infrastructure. Other retains the supplied coarse topic. Raw provider bytes and labels stay in the audit. An exact English translation echo normalizes to null; a differing English rewrite is rejected. These deterministic normalizations neither rewrite originals nor infer additional event claims.

The interpretation contains `event_schema: relic.event-claims.v1` and `events[]` with stable interpretation-local id, type/state/text/quote, exact citation, time phrase, separately resolved locations and optional approximate city point/label. No event IDs claim cross-message incident identity. Absence of injuries, unknown causes and bare questions cannot create occurrence events. Distinct sites stay distinct in those retained records. The current message map displays one common reported city only; full per-event multi-site rendering is deferred.

Events scope matches messages with a reported typed event, and (since 24 October 2026) messages whose main reading has no events but carry a Jev possible event (`jev_hints`; also matched by a type filter of that type in either view, and exempt from the promotion exclusion). Such messages carry `possible_event: {type, confidence, suggested_by:"jev"}` in query items and message detail of the current publication; it is never part of the reading. See validation/JEV_POSSIBLE_EVENTS.md. Otherwise: during upgrade, old readings without `events` retain the existing kind=event/status=reported fallback. A type scope matches that type (and reported state when in Events). All messages includes matching denied/planned/historical/unclear claims. The same predicate governs counts, map, list and exact membership. A type-filtered query suppresses a message point unless a reported event of that type supports that same point: a denied attack cannot borrow a different strike's dot. Scoped query geometry can therefore be null while canonical detail still contains another event's supported place. Current message-level multi-site ambiguity remains unlocated, even when filtering one type. Counts are messages, not deduplicated incidents.

Viewer `/` defaults to Event reports (10 October 2026; before then All messages). All messages uses `?feed=all`; categories use e.g. `?type=strike`. An older `?feed=events` link opens the same view and normalizes to `/`. A compact link written before 10 October with a filter but no `feed` (e.g. `?type=strike`) now opens Event reports rather than All messages; a message link (`?m=`) still opens its exact message whatever the view. Legacy scope JSON defaults to its original All semantics. Reading a filter runs no model or acquisition work. `processing.needs_upgrade` counts eligible current interpretations from older recipes, separately from raw pending/failed counts. Successfully processed older revisions may be evaluated once for the new recipe under the existing policy; failed/uncertain paid requests are not retried under the same version. A newly evaluated recipe can make one new attempt, preserving prior costs/audits and duplicate-attempt protection; no cumulative quota applies.

### Reviewed event vocabulary v3

`web/src/event-taxonomy.json` and `EVENT_TAXONOMY.md` define 30 supported identifiers in seven groups, including the original eleven. The same `event_type` scope and per-claim contract apply. No weapon/casualty/actor subtype is invented from the category. Prompt `relic.message-events.v8` / recipe `message-events-v6` has a reviewed 49-case fictional evaluation. Structural damage is distinct from infrastructure service disruption. One whole JSON code fence is an accepted transport wrapper; extra prose and duplicate keys are rejected. Null-type claims lose unused event time/place pointers, and an event envelope without typed events is demoted to a statement. These conservative changes are recorded alongside unchanged provider response bytes. The evaluation and its development limitations are recorded in validation/INGESTION_GEOGRAPHY.md.


## Initial history and active geography

Initial collection freezes `{started_at,cutoff,seen_ids,pages,complete,stop_reason}` under `collection_states.initial_history`. It accepts at most 100 distinct ordinary messages no older than one calendar month at the start, deduplicates page overlap, commits source bytes/messages/progress atomically, and resumes initial continuations automatically. The three-page turn and 30-page total bound, source/account holds and Stop survive restart. A legacy unfinished catch-up drains under its original contract before this initial interval is established. No retained evidence is deleted by the cap.

Local maintenance routes (not ordinary viewer menus):

| Route | Contract |
| --- | --- |
| GET `/geography` | `{active:object|null}`; active version, attribution, coverage, product counts and manifest hashes. No network. |
| POST `/geography/activate` | `{editions:string[]}`: one or more existing edition directories inside the project root; no count or size cap, only a free-disk-space check. Returns a background job. One existing import worker, cancellable through `/jobs/{id}/cancel`. Failed validation/cancellation retains the previous active pointer. Raw archives, extracted members and indexes are copied/verified before the short pointer transaction. |
| POST `/geography/reresolve` | Empty object; background job using the existing processing and import worker bounds. Reuses exact retained validated readings and their matching audits; zero model calls. Complete batches of at most 50 become new publications. Prior publications, source revisions, text, reading recipe and citations remain immutable. |

`interpretation.reference_edition` identifies the workspace reference version. Internal interpretation uniqueness uses the reading recipe plus geography edition; public `recipe` continues to identify the original reading protocol. Geography-only publications record `processing_mode: geography_only` and `prior_interpretation_id`. Direct lineage includes the reference edition and input manifests. Global and regional entries sharing a GeoNames identity are deduplicated; regional full records supersede the overlapping global subset. Literal feature qualifiers can constrain city versus administrative matches. Canonical model names never substitute for unmatched source mentions. Literal country names remain broad even when a locality shares the name. Coordinates are approximate locality candidates, not observed incident coordinates. Region polygons, routes and full per-event multi-site rendering remain separate work.

### Retained processing-response recovery

`POST /api/v1/processing/recover` admits one bounded local job through the processing worker and existing import slot. It revalidates at most 256 known-cost failed responses for the current recipe/geography, exact source text hash, model and prompt, within the eligible-source policy. No upstream request is made. A recovered attempt retains its original response object/error in `recovery`, preserves cost and request count, and supplies a validated cached reading for ordinary coherent publication. Still-invalid responses remain failed and are marked checked once per recovery version, with `replay_rejection: {version, kind, detail}` naming the current decoder's error kind and failing item/rule (for example `claims[1]: time_text outside quote`; field paths only, never text). New failed response audits carry the same locator as `rejection`. Unknown billing is excluded. The response is an API v1 job with `accepted`, `total`, status and detail. These counters also apply to geography jobs; early saved geography jobs migrate on startup.

An English provider rewrite in `translation` is discarded, retaining the exact original for reading. This removal is recorded in `conservative_normalizations`; unsupported quotations, wrong event links and invalid remaining fields still reject. The original provider bytes are never rewritten.

Resolver policy `relic.geography.v2` abstains when otherwise point-like event candidates in the same exact occurrence quote imply conflicting countries without supporting country context. Candidate records remain inspectable as `context_conflict`; neither a remote homonym nor a nearby guess is promoted. Explicit country-supported cross-border reports and issuer-only context remain distinct. This catches a reviewed metropolitan-neighbourhood mismatch, but does not establish complete contextual geocoding. A policy change requires a new active edition and zero-call re-resolution of retained readings. Changing only the geographic edition never makes a failed semantic model request eligible for another paid call.

Legacy readings may omit their event array or retain `events: null` from an early geographic replay. Both represent no structured event records for the reader; the client normalizes them to an empty array while continuing to reject malformed populated events. Geographic replay now preserves an absent legacy field without inserting null, retaining its existing backend fallback semantics. Immutable old readings are not rewritten. `check_reading_contract.mjs` covers these shapes; its optional `--live-local` mode validated all 2,276 messages in one frozen live publication, plus the workspace response, against the actual frontend schemas. Final browser verification showed the populated live feed without an API schema error.

Normal catch-up now saves `catchup_auto_resume` separately from its durable continuation. Progressing three-page turns resume automatically at the source cadence, including across restart and transient provider failures. Stop, disabled policies, blocked sources, identity conflicts, account cooldowns and repeated-cursor/resource holds remain authoritative. The original watermark advances only with a durable drained interval. Known legacy page-limit pauses migrate conservatively; unknown paused states do not. Initial history retains its separate 100-message/month and 30-page boundaries.

AI attempt metadata can now carry `receipt_generation_id`, `receipt_model`, `receipt_checks`, `receipt_next_at`, `billing_receipt_hash` and `billing_reconciled_at`. The header identity is written before reading completion content and survives cancellation. Read-only receipt reconciliation updates only unknown cost (and changes an interrupted `reserved` status to `unknown`), never the original error/audit, evidence, interpretation or failed-request retry eligibility. Exact ID/model and final-cost validation applies at both protocol and storage boundaries. No frontend setting or new credential is introduced.


## Message sites — 20 September 2026

Query items and publication-pinned evidence detail return `map_locations: [{id, label:string|null, geometry:GeoJSON.Point, approximate:boolean}]`. This is a read projection of retained source coordinates and each supported reported event location. Immutable evidence, interpretations and citations are unchanged. A message may have several sites even when its legacy summary `geometry` is null. Never substitute its first city for its whole meaning.

One message contributes once to total/located/unlocated, list pagination and timeline. `located` means at least one site in the active scope; it does not mean every mentioned location was resolved. `map_loaded` counts rendered sites and may exceed `located`. Feature IDs combine exact evidence-row identity with place identity; properties additionally include `message_key`, `site_key`, `location_label`, `approximate` and the sole `event_type` when a site has exactly one matching type. Claims at the same publisher place identity in one message share a site, without merging incidents across messages. At 10,001 sites the map is truncated to 10,000, `map_truncated` is true and coverage explains the draw cap. Page location links remain complete independently of that cap.

Source/time/text/type predicates select messages as before. The same type and geographic bounds select map sites and list location labels. With bounds, a message is included if any eligible site intersects the box; crossing-antimeridian bounds work. `include_unlocated` adds messages without an eligible site, not messages located only outside the box. A denied, planned, historical, unclear, issuer or destination location never supplies a derived occurrence marker. Genuine raw source coordinates remain available in the untyped map view; they cannot establish an event-type occurrence by themselves.

The full message detail keeps all its supported sites from the selected publication, even when the feed is filtered by one type. Its compact links navigate the camera; they do not change the filter. Selecting a message highlights all its currently visible sites. Cluster badges count sites; cluster choices deduplicate exact messages and disclose when only the first 100 sites were inspected. More than two reader location links stay behind “more sites”. No new processing, provider call or configuration is needed to expose already retained sites.


Performance implementation: supported sites and per-type claim states are indexed once per immutable interpretation in SQLite. An INSERT trigger uses the same SQL projection as the atomic startup backfill, including a marker for interpretations with no sites. Publication joins always select the pinned interpretation. These tables are rebuildable indexes, not independent event truth. No source evidence JSON or saved reading is rewritten; backup restore and older-workspace migration are tested. This replaces repeated location JSON parsing in map/bbox/type queries after the dense fixture exceeded the latency gate.

## Country and administrative areas (20 September 2026)

`GET /boundaries` returns `{active:null|edition}`. `POST /boundaries/activate` accepts `{editions:string[]}` (one or more explicitly selected, root-contained candidate directories; no count or size cap; the three reference-activation routes accept request bodies up to 1 MiB, other control routes 4 KiB), returns a cancellable job, and atomically replaces the active area reference only after hash/count/identity/topology validation. Old shape editions remain available to pinned publications. `POST /geography/reresolve` reuses retained paid outputs against the combined places/areas edition; it makes no model calls.

Each occurrence location can carry `area:{id,label,bounds,level,represented_time}`; its point `geometry` remains null. `map_locations` continues to mean point sites. Evidence also returns `map_areas:[{id,label,bounds,level,represented_time}]`, filtered to the same event type and viewport. `located` counts messages having a point or an area once; `map_loaded` counts only point sites.

The query adds `map_areas:FeatureCollection<Polygon|MultiPolygon>` and `areas_truncated`. Every shape appears once with properties `area_id,label,level,bounds,message_count,choices`; choices are exact `{id,revision,source,title}` records (maximum 100 per area). At most 100 display areas and 4 MiB of display geometry are returned; coverage explicitly discloses truncation while feed/count/timeline membership stays complete. `POST /area-selection` accepts `{scope,id,revision}` and returns `{ids:string[]}` using the same pinned publication/type/time/bounds predicate, independent of the first page or first 100 area choices.

Spatial membership uses full retained analytical geometry, including holes, separate islands and dateline-crossing bounds. Publisher simplification is solely for rendering. A reported country/province is a broad possible location, never a centroid pin or an asserted event footprint. Denied/planned/issuer context does not create an area occurrence. All source bytes, original metadata/rights/represented years, shape identities and feature hashes remain inspectable; derived cache metadata records the analytical source and feature hashes separately.


## Message continuation and admission increment — 20 September 2026

Publication-pinned evidence detail adds `media_context: {members, anchor, limited}`. A member contains exact `id`, `revision`, `published_at`, `media`, and `reason` (`original`, `album`, `reply`, `nearby`). `anchor` is null or `{id, revision, reason}`. Own media stays in the original `media` field; members exclude the selected message. Native album/reply relationships take precedence. Temporal inference is same-source, known-author-compatible, image/video-bearing, <=180 seconds from the preceding substantive ungrouped message (>=12 characters excluding URL/handle/tag-only text), and applies only to a follower without its own report: no substantive text, or at most three words and 40 characters. An explicit same-author reply still attaches with a caption of <=160 characters / 24 words (24 September 2026; applying that caption bound to inference attached separate short reports). Reposts/quotes/forwards and replies elsewhere do not use inference. Same-second numeric provider IDs order deterministically. The lookup considers a bounded 400-message neighborhood, includes directly linked originals/albums/replies, and emits at most 50 companion messages. Overflow is disclosed by `limited`, never silently accepted as complete. Historical publication membership and revisions constrain every result; no interpretation, original, media ownership or map geometry is rewritten.

Queries add `unread` (pending nonempty text from sources selected for reading; see "Reading choice per source"), `not_selected` (the same from other sources) and `media_only` (pending empty text), excluding removed upstream records and using the same scope as total/map/feed/timeline. These describe reading state, not queue eligibility or event truth. `unlocated` remains the total without a supported map site/area and overlaps unread/media-only. The ordinary status prefers a compact unread count; coverage explains the separate figures.

Processing `today` adds `admission_reserved_usd`, `next_request_reserve_usd`, and `spend_limit_usd`. `today.limit` and `today.spend_limit_usd` are null in both lanes: there is no cumulative request or spend cutoff. The legacy-named reservation fields are accounting estimates only and never block work. `cost_usd` remains null until every cost is known. Transactional attempt admission refuses another paid attempt for the same evidence revision and semantic recipe. No receipt lookup is treated as a retry of generation.

Boundary review/activation recognizes only the exact assets in `reference/reconciliations/geoboundaries-irn-adm1-17685810.json`. The accepted manifest and each derived shape retain this reconciliation beside unchanged publisher metadata. Full/display identity and topology checks remain mandatory. The independent pinned TopoJSON corroboration contains the same 32 IDs; no shape or administrative identity was merged or invented.

## Supplementary decisions — 21 September 2026

`PUT /processing/supplemental` accepts exactly `{enabled:boolean}` and activates/deactivates the pinned advisory recipe. Source eligibility and processing Pause/Stop remain authoritative. `GET /processing` adds `supplemental` with enabled/running/completed, text-size exclusions, daily usage accounting and an explicit latest notice. Neither lane has a daily request quota or spending cutoff. Pause/Stop, source eligibility, actual provider limits and per-request resource bounds remain.

Evidence detail adds nullable `supplemental`: exact result ID, model, phase, timestamp, request/audit object hashes, typed choice/probability/confidence map and input context (exact message/neighbour revisions, candidate IDs, source interpretation, omission counts). Live views may show a saved initial suggestion or an enrichment matching the selected reading. Historical views exclude results created after their publication; results never cross evidence revisions or incompatible reading IDs. Requests/responses and source/reading input edges are retained. No supplementary records participate in membership, filter, map, media or baseline-extraction tables. Requests with malformed choices, unknown candidate keys, incomplete answers, invalid probabilities, a mismatched served model or mismatched receipt hashes remain failures, not usable suggestions.

See JEV_IMPLEMENTATION.md for resource bounds and quality boundaries. All X/Flightradar integration traffic and protocol contracts remain unchanged.

Supplemental status also exposes `request_excluded`: saved preflight failures for combined prompts exceeding their bound. They create no provider request or baseline exclusion; later messages continue.

Geographic policy v3 preserves the location DTO: `resolved_area` may represent the common containing province of city/province alternatives or an explicitly supported country containing an unresolved local feature. `precision`, `area.level`, `reason` and candidate `alias_reference` disclose the retained alternatives and supporting quote. This does not imply a resolved local site. Geographic-only replay retains semantic event states, exact originals, prior publications and the reading recipe.


### Compact reading presentation (21 September 2026)
Map point previews and area choices include `published_at`, `acquired_at`, compact `events:[{type,state}]` and `processed`. Map preview text uses the publication-pinned concise English headline, with translation/original fallback for legacy or unread records; exact originals stay in evidence detail. Point previews are bounded to 400 characters, area choices to 600 and 100 newest messages per area. `map_areas` and area features include the accepted boundary’s ISO `country_code`. The reader/feed derive city country names only from selected accepted candidates. Area counts mean messages supported somewhere in a boundary, not precise sites or independent incidents. Exact IDs/revisions remain the selection keys.

The viewer hides the confirmed Rainbet sponsor footer and standalone @wfwitness signature from readable presentation. Exact source text, translations, citations, exports and Original are unmodified. Event labels remain extracted baseline claims; supplementary Jev suggestions do not overwrite them. Publication timestamps (receipt time explicitly labelled when missing) and relative ages share one ticking viewer clock.


Translation-copy repair (21 September 2026): a non-English reading whose purported translation exactly copies a source containing at least three non-Latin alphabetic characters is rejected for new responses. The existing enabled baseline worker admits such retained copies in the shared concurrent queue for one full-reading repair each, with a distinct `message-events-v6/translation-echo-repair-v1` receipt key. Source eligibility, length/removal gates, cost accounting and cancellation still apply. Cached untranslated copies are not reused. Each revision receives at most one repair attempt; failed/uncertain repairs remain recorded and do not loop or rebill. Old interpretations, original text and historical publications remain intact. This narrow check detects untranslated copies, not general translation accuracy or completeness.

Confirmed untranslated copies use the already-saved English summary in readable presentation and explicitly say **English summary · Original** in the reader; they are never labelled as translated. This is a presentation fallback while a repair is pending or has failed, not a claim of complete translation.

Processed feed rows use the same concise English headline as map previews. Opening a row reveals the full saved translation or cleaned original; shortening previews never changes the underlying text or citations.

Concurrent processing (21 September 2026): `limits.batch=256`, `limits.concurrency=64`; supplementary status also exposes `concurrency=16`. These are simultaneous resource bounds, never cumulative usage quotas. Validated receipts are retained individually and published coherently. Provider failures stop refill and drain started requests; Stop cancels them with truthful unknown billing. Successful automatic batches immediately continue.

Supplementary result commits emit change kind `supplemental`; they affect processing status and message detail, not ordinary map/feed/workspace contents. Clients coalesce live refreshes over 750 ms. Intermediate processing job progress is persisted/emitted at most once per second; final job and publication events remain immediate. Provider backoff survives restart, while local paid validation failures and explicit cancellation do not impose a new provider cooldown.


### Reading queue and quality review — 21 September 2026

`GET /processing` retains `pending` as current revisions without a published reading; it is **not** the dispatch queue. `queued` and `in_flight` use the same eligibility query as dispatch. `unavailable` contains unread `removed`, `without_text`, `ineligible` and `oversized` counts. `failed` counts blocked work, including an update that retains an older reading, so it is not necessarily disjoint from `processed`. `needs_upgrade` counts queued/active updates with a previous reading. These fields must not be combined by subtracting overlapping historical failures. `corrections_queued` is a subset of queued work; it stays zero without a passed correction evaluation.

An explicitly reviewed failed baseline evaluation makes `evaluated:false` and supplies `quality_notice`. Scheduling authorization remains saved, but the existing evaluation gate prevents new baseline generation. Collection and retained publications remain usable; cost, quota and source authorization are separate states.

The active correction implementation (`message-events-v6/correction-v8`, `relic.reading-correction.v8`) requires a matching reviewed evaluation in `processing_correction_evaluation`. It sends the same evaluated full reading request as new work; its separate admission and receipts permit one new request only for known-cost retained schema/quote/translation failures. It rechecks original/response hashes and stores `correction_of` and the previous audit hash. Unknown billing, provider failures, removed inputs and successful current readings do not authorize correction. Failed/cancelled corrections cannot loop.

Both readiness and execution compare `request_profile`, including the request fingerprint and decoder version. A passed report for another provider/prompt/schema/reasoning policy cannot enable work. A stale profile supplies `quality_notice`; no new UI setting is required. The owner-selected profile is GPT-6 Luna/OpenAI through OpenRouter, prompt v11, decoder v5, explicit reasoning effort medium and no temperature parameter. DeepSeek/Baidu remains compatible for historical workspaces. Raw responses and originally served providers remain unchanged during replay.

Decoder v2 may repair one place-array pointer using a unique existing place that appears in the exact claim quote and is named in its claim text. It does not create or drop places, claims, support or coordinates. Audit marker `exact_quote_place_index:<claim>:<old>-><new>` records the adjustment. All ordinary validation still runs at the publication boundary. Already-paid current-prompt baseline and correction failures can be replayed once per decoder recovery version without another request; the previous failure/audit/receipt remains retained.


Reading repair continuation (22 September 2026): substantial mixed-script prose with a legacy `en`/null translation is explicitly incomplete. It enters the same once-only repair queue as copied translations; `queued`, `in_flight`, `failed` and `needs_upgrade` include these outcomes even when an older reading remains available. Previously accepted incomplete cache entries cannot bypass a full repair. Failed/unknown repairs preserve older readings and never become automatic retry loops.

Ordinary v11 model requests are unchanged. The conditional `relic.mixed-reading.v1` instruction is separately fingerprinted in `request_profile.mixed_script_request_sha256`; source bytes remain the unchanged user-role input. Supplementary Jev does not participate in this decision or replace baseline extraction.

Exact-citation decoder v3 permits unique ASCII layout alignment only: runs of space, tab, CR and LF map to existing separator runs. No character, punctuation, number, negation or word-boundary insertion/deletion is accepted. `QuoteSpan.layout_alignment` defaults to false for older audits; aligned spans return exact original text/scalar offsets and `exact_source_layout_alignment:{claim|location}:{index}` audit markers. This changes matching, never evidence bytes. Ambiguity and unsupported associations still reject. Explicit `recovery_review.decision=hold` on a saved failed attempt blocks offline promotion for a documented semantic concern.

Readable fallback uses the existing English headline with **English summary · Original** only while a detected translation is unavailable. Full repaired translations remain available in ordinary reading and may retain equivalent bilingual repetition; the client never assumes a foreign-language paragraph adds no information.

## Review corrections · 22 September 2026

- Publication membership and interpretation membership are now validity intervals
  over internal publication ordinals. Public IDs, exact revisions, frozen scopes,
  full `/query` and `/evidence` contracts remain unchanged. Migration preserves
  every historical membership, including removal and re-addition; no history is
  pruned. Ordinals are internal and are not timestamps.
- `POST /query/preview` accepts the same request/scope as `/query`. It returns
  `representation:"preview-v1"`, identical counts, IDs, pagination, sites and
  timeline. Preview readings omit full claims and processing metadata; events
  include type/state and only selected geographic candidate display fields.
  These are display summaries, never full analysis records. `/evidence` still
  returns the complete publication-pinned reading and exact original. Since 9
  October preview map points omit `message_key`, `location_label`, `approximate`
  and `event_reports` (unused by the map; `/query` keeps them).
- Performance (9 October 2026): one scan of the scope feeds counts, timeline, page
  order, cursor and related counts; later steps reuse its message set inside one
  read transaction, so every figure comes from the same snapshot. A trigger-kept
  `interpretation_previews` table and the `interpretation_interval_reading` index
  replace per-row reading JSON parsing (built once on the first start, about 3 s
  live). Map-area scopes test an outline only when its stored bounding rectangle
  can touch the view (`relic_bounds_may_intersect`). Equivalence tests keep the
  previous implementation as a reference over 20 scopes. Live: default preview
  0.24 s (was 1.8–2.1 s); on a snapshot every measured scope is 55–340 ms.
  `GET /processing` keeps a per-workspace memo of each message's classification,
  reclassifying only messages whose evidence, reading or attempts changed (debug
  builds compare it with a full pass on every refresh): 63 ms (was about 650 ms).
  The panel polls every 4 s while reading runs, and event-driven refreshes are
  limited to one per 2.5 s.
- Preview area features carry `geometry:null` and include every area in scope
  (no area-count or byte cap since 27 September 2026; the preview never reads
  outlines, so `areas_truncated` is false). `GET /area-shape?id=…` returns
  `{id,geometry}` for the immutable edition-qualified retained area. The frontend
  fetches at most four concurrently and caches by that identity. It exposes a
  new query result only when all required shapes resolve; failed shapes cannot
  silently disappear from the map or its counts.
- `GET /processing/failures?before=<rowid>` lists up to 50 distinct messages in
  the **current failed work state**, newest rowid first, with `next_cursor`.
  Items expose exact id/revision/message_key, source label, publication time,
  bounded original preview, latest failed/unknown reason and `retained_reading`.
  It does not count historic attempts as distinct messages or initiate retries.
- `GET /source-attention` returns `{count,sources,layers}` (9 October): `count` is
  sources plus degraded live layers; `layers` lists `{id,label,state,detail,since}`
  for `ships` (KeelWatch unreachable, unhealthy, refusing the token, or not
  collecting — an ongoing collection gap or no run for 45 minutes; checked at most
  once a minute with a 2 s limit, unknown never counted except two timeouts in a
  row), `warnings` (MSCIO's last poll failed, last success over 30 minutes old, or a
  suspicious list; only where polling is allowed) and `aircraft` (every
  Flightradar24 proxy resting, or the provider's 120 s rate-limit rest; nothing is
  sent to check). `GET /layers` cards add `health:{state,detail,since,checked_at}`
  (ok, degraded, unknown, not_configured, off) and a `warnings` card.
  `GET /layers/warnings[?days][&at]` features add `listed`, `unlisted_since` and
  `last_listed_at`: after a complete successful MSCIO poll, a retained alert that is
  no longer listed is "No longer listed by MSCIO since <time> — not a cancellation";
  it returns to listed when MSCIO lists it again. Changes are recorded in
  `mscio_listing` / `mscio_listing_changes` (each linked to the retained response);
  evidence rows are never changed. Failed, malformed or empty lists, rows without
  an AlertId, and a list unlisting more than half of ten or more listed warnings at
  once (held until the next identical poll) change nothing. With `at`, listing is
  as recorded at that moment.
- Source counting (unchanged): sources count for enabled, configuration-current
  sources whose saved state is needs_attention/blocked/error, whose catch-up is
  delayed, or that have not collected successfully for 10x their poll interval
  (at least 30 minutes) for any reason, e.g. a down proxy. Subscription
  `collection.stalled` marks the same condition and Sources shows "Nothing
  collected since …". It uses reserved status capacity; the map links this
  count to Sources.
- Ordinary reads retain four workers, with at most 16 waiters and 250 ms wait.
  Status/cancel have two separate workers and at most eight waiters. Excess
  admission still returns `capacity_exceeded`; shutdown rejects queued work.
- An All timeline chooses the shortest of 7, 30 or 90 days containing at least
  80% of dated activity as its chart domain and returns `earlier` and `earliest`. `earlier + undated +
  sum(buckets)` equals total scope membership. An explicit date range is never
  shortened. The earlier count opens its exact interval; no messages are hidden
  from the All feed or map.
- Successful collection pages containing only already-retained exact revisions
  update source/checkpoint state with `last_check:{at,unchanged:true,records}`.
  They return `unchanged:true`, null job/input hash, and reuse the publication.
  No new raw acquisition, per-message lineage edges or page job is claimed.
  Successful unchanged source-operation jobs are removed only when they have
  no lineage node; failed/partial/acquiring operations retain their ledgers.

Native Telegram albums share a feed row within the loaded scope/page set. The
caption remains readable and each exact member is available from an Album menu.
The footer distinguishes loaded messages from rendered rows; counts, map and
pagination still count messages. No inferred temporal or semantic merge occurs.

## Second operational review corrections — 22 September 2026

- POST `/query/page` uses the ordinary query request but requires a cursor and an
  explicit `scope.publication_id` from its first page. Returns only
  `{scope_revision,scope,publication_id,generation,loaded,items,next_cursor,representation}`;
  items use the compact preview shape. It does not recompute map geometry, scope
  totals or timeline. The client retains these from the matching first page.
  A different cursor/scope/publication is still rejected; errors remain visible.
- Retained-media responses use eight separate streaming permits; paused media cannot
  occupy the four query workers. Disposal/disconnection releases capacity.
- `/processing` adds nullable `retry_at` and reports adaptive concurrency. Explicit
  pre-generation 429 rejection is deferred, not a permanent message failure. The
  receipt, zero-cost rejection basis and shared cooldown are durable. Ambiguous
  failures are not automatically retried; see CONFIGURATION.md.
- Subscription `collection` adds `can_rebaseline`, `gap_recorded`, `delayed`.
  POST `/subscriptions/:id/collect` with `action:"baseline"` and the current
  `expected_version` restarts an incomplete X interval only. It atomically saves
  the prior checkpoint/state in `collection_gaps`; no evidence is removed.
  `/source-attention` includes prolonged incomplete intervals, even between turns.
- Backup manifests remain schema v1 and include every snapshot object. The former
  total object/byte cutoff is removed; manifests and file hashing/copying stream.
  Backup generations may hard-link verified objects from one another, never live
  objects. POST `/backup/:id/verify` verifies in disposable storage and returns
  `restored_directory:null,verification_copy_retained:false`; explicit Rust restore
  callers retain the prior `verify_backup` behavior.
- Successful versioned area shapes and content-hashed JS/CSS/font assets use private
  immutable caching. Mutable data, HTML and errors remain `no-store`.
- New interpretation lineage nodes reference exact retained interpretation/audit
  objects instead of duplicating the reading JSON. Reused readings link the earlier
  exact evidence revision with `reading_reused`. Acquisition edges cover newly
  retained revisions; full raw pages still retain repeated input records.
- Supplementary change events identify `evidence_id@revision` in `affected_ids`, so
  unrelated Jev results do not reload the open reader. They never change map scope.


### Retained-response repairs and geographic context · 23 September 2026

Decoder v5 retains v3 exact-layout alignment and adds narrowly audited recovery:
unique wrapper-quote/terminal-period/initial-ASCII-case presentation differences;
literal Arabic attached-preposition matching; repeated place excerpts selected by
one consistent exact linked-claim passage. An unlinked `mentioned` place with a
repeated excerpt may instead cite the complete original when it fits the existing
1,600-character quote bound. Event/issuer/origin/destination ambiguity still rejects.
Unsupported context-only claim links can be removed while retaining every place,
claim, translation and raw output. Unsupported event-site links still reject.
All resulting quotes and offsets refer to actual original bytes; no fuzzy paraphrase
matching, invented headline or silently discarded substantive text is introduced.

A successfully replayed baseline can publish despite a later failed correction.
Once a timed-out generation has an authoritative matching, finished billing receipt,
one evaluated correction is eligible; uncertain costs, missing receipts and repeated
corrections still block that retry. No daily quota or cumulative spending cutoff.
Identical-text revision reuse tolerates decoder-version changes only after current
revalidation; all generation-profile settings must still match.

Combined geography policy `relic.areas.v3` includes accepted place, administrative
and marine editions. Re-resolution reuses existing model readings and costs no model
request. ADM1 crosswalks may use exact retained publisher aliases, subject to country,
feature hierarchy and full-polygon containment. Literal hashtag/administrative
qualifiers and immediately attached country phrases are recognized conservatively.

Selected SeaVoX marine shapes use existing `map_areas`, `/area-shape` and exact scope
membership, with `level=MARINE`, no country assignment and no centroid point. They
are reference areas, not observed incident footprints. Original source claims and
non-occurrence role/state checks remain authoritative. Acquisition/activation is
explicit offline maintenance; there is no new normal-view settings panel.

### Selected OSM facilities · 23 September 2026

`GET /osm` returns `{active:null|edition}`. `POST /osm/activate` accepts
`{editions:string[]}` (one or more existing root-contained candidate directories; no count or size cap), returns
an existing cancellable import job and atomically replaces the selected reference
set. Limits and native acquisition/rebuild commands are in CONFIGURATION.md.
`POST /geography/reresolve` reuses saved outputs; it does not bill new readings.
Mapping `relic.osm-features.v2` (27 September 2026) adds named places; their outlines are `map_areas` with `level:"PLACE"` and their points approximate settlement precision. v1 editions remain readable. The context identity includes OSM only after activation. Exact current names,
accepted full-country containment, role and occurrence gates all apply. Points
remain approximate references; polygon locations use existing `map_areas` with
`level:"FACILITY"`, retaining holes and historical shapes. Lines stay context-only.
Per-extract snapshot time and object modification time are distinct provenance.
No new ordinary navigation, default facility-marker wall or automated download.


## Typed event relationships — 23 September 2026

- `PUT /processing/relationships {enabled:boolean}` activates/deactivates the compiled,
  evaluated relationship profile and returns its status. Source eligibility and
  processing Pause/Stop still apply; this cannot enable baseline processing.
- `GET /processing` adds `relationships`: enabled, completed pair attempts, in_flight,
  failed, retained links, known_cost_usd, unknown_cost_requests, projection_errors,
  nullable last_error, running. In-flight reservations are not unknown charges.
- `POST /event-links/query {id,revision,scope,offset?:number}` returns
  `{publication_id,total,next_offset,items:[{evidence,links:[{id,relation,direction,peer_state,peer_claim}]}]}`.
  Twenty messages per page, newest first; `total` counts distinct peer messages,
  not claim edges. Relation is `same_occurrence|update|denial|correction`;
  direction is `earlier|later|contemporary` relative to the open message's claim.
  Equal stored millisecond times use contemporary wording. `peer_state` determines
  whether the peer is the denial or the disputed report. `peer_claim` is the
  linked derived claim used for the compact preview, so long messages show the
  relevant passage; opening the message still shows its full saved reading.

Both endpoints of a visible link must pass the exact map/feed predicate and match
their reading revisions in the selected publication. A link cannot appear before
its own publication ordinal. Superseded readings hide old links from current views
without altering older snapshots, receipts, originals or lineage. No incident merge,
feed filtering or independent-origin promotion is implied; the event group below is
a reader view over these same visible links.
The compact reader replaces its old city/topic/day navigation with these links;
legacy `related_key` query compatibility is retained.

## Semantic event retrieval and event groups — 26 September 2026

- Retrieval `relic.event-candidates.v3` keeps claims without exactly one resolved
  place. Each claim text is embedded once (`openai/text-embedding-3-small`, 1,536
  dimensions, OpenAI only), keyed by the prepared text, so unchanged re-readings
  reuse the vector. A claim's four closest reports (cosine ≥ 0.55) within six hours
  become extra candidate pairs when neither side has a *different* resolved place.
  Similarity only selects candidates; the unchanged Luna pair review decides every
  link. A place resolved on one side only is withheld from that review request, so
  it is not read as a mismatch. Same-place and placeless requests are byte-identical
  to before; retained decisions are reused without new charges.
- `GET /processing` `relationships` adds `stopped` (the lane is held by an error
  until the owner resumes it) and `semantic`: `{model, requests, failed, vectors,
  candidates, known_cost_usd, unknown_cost_requests, last_error}`. Top-level
  `known_cost_usd`/`unknown_cost_requests` include the embedding requests.
- Event groups (27 September): connected components of the links visible at the
  selected publication between claims of current readings, restricted to in-scope
  messages. Claims, not messages, are joined, because one message can report several
  events. Query rows add optional `event_group:{id,reports}` (the message's largest
  group in this scope) and `event_links` (reports - 1). Map point properties add
  `event_group` and `event_reports`; the map draws one marker per event per site
  and lists its reports on click. The feed folds same-event rows under the newest.
  Rule links (26 October 2026, owner: obvious repeats were left apart) join the
  same components without a model call: reported claims of one event family
  (strike, armed attack, explosion, weapons launch, air defence and structural
  damage are one "attack" family; other types only themselves) that share a place
  key (an exact site, or a province, sea or other sub-country area, never a
  country alone), each within 3 hours of the previous, chains capped at 12 hours.
  A reviewed `distinct` decision for a pair vetoes its rule link; claims of one
  message are not rule-linked. `POST /event-links/group` (the reader's timeline)
  still shows reviewed links only.
- `POST /event-links/group {id,revision,scope}` returns
  `{publication_id,groups:[{claim:{id,text,type},members:[{evidence,anchor,claims}],links:[{a,b,relation,a_state,b_state}],truncated}]}`,
  one group per event the anchor reports. Members are reachable from that event's
  claim, oldest first, at most 60 (`truncated` marks more). Only in-scope messages
  with their current reading are members or bridges. Member evidence carries
  `map_locations`/`map_areas`. An empty `groups` array means no visible links. A
  group is a view of suggested links, not a merged incident or independent
  confirmation.
- `relationships.links` counts links between current readings only; links of older
  reading versions stay in history.
- `GET /processing` adds nullable `retry_reason` (`credits|rate_limit`) beside
  `retry_at`. An OpenRouter pre-flight credit rejection (HTTP 402, error body only,
  no provider/usage) is a zero-cost deferral in every paid lane, never a failure.
- Embedding requests are admitted durably before dispatch. A restart turns a
  reserved request into unknown cost and holds the lane; resuming never resends it.


Qualified location mentions may now return `status: ambiguous`, multiple retained
`candidates` and `literal_qualifier` with `locality_id: null`. This means the literal
parent country/province narrowed the alternatives but did not establish one local
identity. It is not a point placement. Context v6 versions this change for replay.

## Processing review increment — 25 September 2026

- `POST /processing/retry-interrupted` (no body) authorizes one new paid attempt for each current message held only by an interrupted request (`unknown` outcome), then starts a run when processing is enabled and idle; otherwise the automatic loop admits them. The original attempt, reservation and any reconciled cost remain; the authorization is recorded as `retry_authorized_at` on that attempt. A retry cut off again is held again. Nothing retries unknown outcomes automatically. Returns processing status.
- Processing status adds `interrupted`: current messages held by an unauthorized unknown outcome (included in `failed`).
- Shutdown drains: SIGINT/SIGTERM stops admitting work, lets reading, Jev and event-link requests already sent finish and be recorded (at most two minutes; a second Ctrl-C stops at once), and only then cancels. Startup converts any reading reservation left by an exit into an `unknown` outcome, matching Jev and event links; receipt reconciliation still applies.
- Event-link status `last_error` reports only failures newer than the latest completed comparison; `failed` still counts all.
- `/query/preview` items omit reading internals `support_alignment`, `related_meaning`, `event_schema` and `prior_interpretation_id`; `/evidence` keeps full readings.

## Live context layers — 27 September 2026

Read-only views; nothing here publishes evidence. The browser never contacts
KeelWatch or Flightradar24. Live by default; at a past moment (chosen in Layers,
sent from the reader, or a historical snapshot's own time, snapped to ten minutes)
ships come from KeelWatch's past tiles, warnings are those registered by then, and
aircraft are hidden because Relic keeps no aircraft history.

Errors (9 October): a provider that fails or answers badly is HTTP 502, one that
is resting after failures or a rate limit is 503 (both `provider_unavailable`), an
unconfigured layer is 503 `not_configured`, and a KeelWatch 404 stays 404
`not_found`. A Flightradar24 429 or gRPC RESOURCE_EXHAUSTED rests every proxy for
120 s instead of rotating proxies against the limit; other transport and HTTP
failures, and gRPC UNAVAILABLE or DEADLINE_EXCEEDED, rest only the failing proxy for
60 s. Since 26 October any other provider answer (malformed, or another gRPC status)
rests none, and a flight detail the provider does not keep (gRPC NOT_FOUND, as for
ground vehicles) is 404 `not_found`. The browser retries a failed aircraft view after 30, 60,
then 120 s and fades the last aircraft as stale; a failed ship tile is named in the
Layers panel ("not received, not absent"), and the ship view is dated when tiles
arrive. Ship tracks are not cached; other KeelWatch, aircraft and photo caches are
bounded by bytes.

- `GET /layers` lists `{layers:[{id,provider,configured,family,temporal,coverage}]}`.
  An unconfigured layer explains which private setting it needs.
- `GET /layers/ships/tiles/{z}/{x}/{y}[?freshness=30m|2h|24h|7d|all]` passes
  KeelWatch's vector tile (`/api/v1/tiles/{z}/{x}/{y}.mvt`, Mapbox Vector Tile,
  layer `vessels`, zoom 0–14) through Relic, so the browser never learns the
  private service address. Up to zoom 5 a feature may stand for `n` ships sharing
  half a screen pixel, with light properties (`cat`, `sog`, `dir`, `age`, `len`,
  `sat`, `n`); from zoom 6 every ship is its own feature with full properties
  (`id`, `mmsi`, `imo`, `name`, `type`, `flag`, `callsign`, `dest`, `nav`, `cog`,
  `hdg`, `ts`, `len`, `wid`, `est`, `src`, `identity`). The feature id is the MMSI;
  the detail key is `mmsi:<id>`, or `id` without an MMSI. 204 is an empty tile.
  `Cache-Control: private, max-age=20`. KeelWatch rebuilds a tile at most every
  30 s and may serve one up to 90 s old while the next is built; the map requests
  fresh tiles every 30 s, so the newest positions shown are 30–60 s behind
  KeelWatch (whose sources update every 2–15 minutes). Category and movement filters
  apply in the map. Merged features are presentation, never observations.
- `GET /layers/ships/search?q=` (2–80 characters) uses KeelWatch's ranked search
  (`/api/v1/search`, 1 October): exact names, MMSI and IMO score 1.0, exact call
  signs and former names 0.95, prefixes, substrings and similar spellings lower.
  Returns up to 20 `{id,name,type,category,flag,mmsi,imo,callsign,lon,lat,observed_at,sog,cog,heading,nav_status,destination,length_m,width_m,match,matched,score,names}`.
- Past moments (2 October): the same tile route with `at` (an RFC 3339 moment, not
  in the future; the map snaps it to ten minutes) and `window` (`1h`–`24h`, the map
  sends `2h`) returns KeelWatch's past tile: each vessel at its last stored fix in
  the window up to `at`, merged up to zoom 5 like live tiles, with `course`/`heading`
  where reported; `age` is minutes before `at` and `ts` the fix time. `freshness` is
  ignored. No vessel cap and no view-width limit. KeelWatch's own cache lifetime is
  passed through (`private, max-age=1800` once the moment is over six hours old).
  Anonymous satellite contacts keep no history and are absent.
- `GET /layers/ships/coverage?from&to` (at most 92 days) returns
  `{from,to,gaps:[{from,to,seconds}],gap_seconds,collecting_seconds}` from KeelWatch:
  stretches of 45 minutes or more in which no collection run started. The map names
  the gap containing a past moment ("KeelWatch was not collecting from … to …");
  ships missing there were not observed, not absent.
- `GET /layers/ships/{id}/names` returns every name KeelWatch recorded for the
  vessel with source and first/last time; `GET /layers/ships/{id}/messages[?mmsi=]`
  returns the messages the ship finder links to it (confirmed, or suggested and not
  rejected), matched by key or MMSI.
- `GET /layers/ships/{id}` returns `{vessel,acquired_at,stale,error}`;
  `GET /layers/ships/{id}/track?hours=1..720` returns KeelWatch's original
  observations `{items,meta,from,to}`. The map joins fixes only across plausible
  steps (gaps over 3 h or implied speeds over 60 kn stay open).
- `GET /layers/aircraft?west&south&east&north[&services=0,1,…]` tiles the live feed
  through the configured proxy (at most 5 levels, 16 requests per view) for the
  selected Flightradar24 service categories (passenger 0, cargo 1, military and
  government 2, business 3, general aviation 4, helicopter 5, balloon 6, glider 7,
  drone 8, ground vehicle 9, other 10, uncategorised 11; all when omitted; the legacy
  `military=true` means `[2]`). Each flight has observed position/altitude
  (ft)/ground speed (kn)/track/time, the provider's silhouette `icon` class and
  supplied flight, registration, type and route fields. `military` is true when the
  provider lists the flight as military or government (a second, small request for
  category 2), `null` when that request failed; it is never guessed from the type.
  `meta` has `requests`, `possibly_truncated`, `military_known`, saturated/unvisited
  bounds and the proxy index. The same view is reused for 15 s; a failed proxy is
  avoided for 60 s and another configured proxy is chosen; there is never a direct
  connection. The browser refreshes a view every 10 s, or every 3 s per provider
  request for wide views, and requests nothing below zoom 2.
  Since 26 October each flight also has `status` (the feed's own flight status
  number), `emergency` (true when that status is the provider's emergency value,
  which its schema ties to squawk 7500, 7600 or 7700) and `trail_from`
  (`[lon, lat, ms]`, the oldest point of the provider's few-second position
  buffer, or null). The anonymous feed carries no squawk code and no vertical
  speed: a live check that day found the extra field-mask paths `squawk` and
  `vspeed` accepted but empty, and `LiveFlightsStatus` without squawks.
- `GET /layers/aircraft/top` (26 October) returns Flightradar24's most-tracked
  flights now, at most 10, ranked by its own count of current viewers
  (`live_clicks`; `total_clicks` too). That is attention on Flightradar24, not
  operational significance. Each item has `rank`, `flight_id`, `callsign`,
  `flight_number`, `registration`, `aircraft_type`, `description` (the provider's
  full type name), `origin`/`destination` (IATA) with `origin_city`/
  `destination_city`, `icao_hex`, and, read back by its exact flight ID (one
  selected-flights request per six; the provider refuses eight), `positioned`,
  `lon`, `lat`, `altitude_ft`, `ground_speed_kt`, `track`, `on_ground`,
  `observed_at_ms`, `icon`, `source`, `status` and `emergency`. Empty provider
  strings are null; a flight not found has `positioned: false` and null position
  fields, never invented ones. `meta` has `requests` (1 to 3), `limit`,
  `positions_complete` and `positions_error`. The list is reused for 60 s; a
  failure serves the last list with `stale: true` and the `error`, or fails (502
  or 503 `provider_unavailable`) when there is none. Same proxy, rest and
  rate-limit rules as the view. The Layers panel asks for it only while it is open
  with aircraft on, at most once a minute.
- `GET /layers/aircraft/search?q=` (2–12 letters, digits or hyphens) looks the
  identifier up worldwide as an exact callsign, flight number and registration (three
  provider requests, each with the provider's own filter) and returns matching flights.
- `GET /layers/aircraft/{flight_id}/photo` returns the aircraft photo the provider
  lists for that flight, fetched through the configured proxy (media hosts
  validated), with `Cache-Control: private, max-age=3600`; the flight detail carries
  the photo `credit`. The browser never contacts the image host.
- `GET /layers/aircraft/{flight_id}` returns airframe (with operator and country of
  registration), schedule, progress (distance flown and remaining, ETA), photo
  credit, current position and the provider trail for one selected flight.
- `GET /layers/warnings[?days=N]` returns MSCIO warnings as point features
  `{id,revision,registered_at,text,type,reference,alert_id}` (latest retained
  revision of each; `days` filters on registration time) plus the poller `status`
  `{checked_at,last_success_at,ok,warnings,new,unchanged,rejected,error}`. Unlike
  ships and aircraft, warnings are retained evidence (below); this route is their map
  view.

### MSCIO maritime warnings

Relic polls `https://mscio.eu/alerts/filter` directly (it is neither X nor
Flightradar24) at startup and every 10 minutes, and retains each warning as
`institutional_advisory` evidence of source `mscio:alerts` through the ordinary
import path. A warning's `id` is `mscio:alert:{AlertId}`; `published_at` is MSCIO's
registration time (zone-less at the source, read as UTC, recorded as an inference in
`advisory.time_basis`), not necessarily incident time. Evidence keeps the issuer
text, `advisory {issuer,alert_id,type,type_code,reference,registration_date_original,time_basis}`,
the exact `provider_record` and the SHA-256 of the whole response, which is stored
as an object.

MSCIO moves every position by up to about 5 km on each request (measured: all 135
warnings moved between two requests seconds apart, median 2.2 km, nothing else
changed). The revision is therefore the SHA-256 of the record without
`Latitude`/`Longitude`; a new revision is retained only when anything else
changes. The position is the one served when that revision was first retained, with
`location_precision` "MSCIO map position, shifted by up to about 5 km on each
request". Before this was known, one restart on 27 September retained a
position-only second revision of each of the first 135 warnings; they stay as
history and carry the earlier "MSCIO reported position" wording. A malformed row is
rejected and counted, never repaired; a list without `data` fails the poll. A
fictional workspace refuses real warnings.

Warnings are not read by default: reading is the owner's per-source choice below.

## Location context v15 — 27 September 2026

Location resolutions keep their shape. New values: a candidate `match_basis` of
`spelling_variant` (one close spelling of a settlement in a known country; the
reason names the gazetteer spelling and the weighted difference) and a
`literal_qualifier` of `{"message_country": "LB"}` when a name was looked up in the
one country shared by the message's other accepted places. Candidate lists hold up
to 100 identities (was 20). An OpenStreetMap border crossing lying in two countries
has a `null` candidate country. See `CONFIGURATION.md` for the rules.

## Ship finder — 27 September 2026

Suggestions of which KeelWatch vessels a message names. Nothing here changes a
reading, and nothing is linked until the analyst confirms it.

- `GET /evidence/vessels?id&revision` returns `{detection,lookup,decisions,reading_allowed,matches,identifier_matches,profile}`.
  `detection` is free and runs on every message: `{likely,words,identifiers}` from
  vessel words in English, Arabic, Persian, Hebrew and Russian, and IMO numbers
  (with a valid check digit) or MMSI numbers written next to their label.
  `identifier_matches` look those identifiers up in KeelWatch without any model
  request. `lookup` is the retained extraction, or null.
- `POST /evidence/vessels` `{id,revision}` runs the extraction once, on request:
  one strict-schema request to `openai/gpt-6-luna` pinned to the OpenAI provider
  without fallback, profile `relic.vessel-mentions.v1`. Each vessel is returned with
  its name as written, an exact quote (verified against the original, with
  character offsets), a Latin spelling for search, and identifiers, flag or type only
  when written; a name outside its quote rejects the answer, and an identifier
  outside it is dropped. Request, raw reply and receipt are retained objects; cost
  is recorded. A completed extraction for the same revision and profile is reused,
  never resent; a failed or unknown one can be retried. Attempts interrupted by a
  restart become `unknown` and are not resent. The source must be selected for
  reading, because this sends its text to the model.
- Matching is free and recomputed on each read: IMO or MMSI lookup when written;
  otherwise KeelWatch's ranked search (`/api/v1/search?limit=200`, 1 October)
  without ship prefixes. Candidates are hits scoring at least 0.95: the same name,
  call sign or a former name (basis "reported as X by source until date"); hits
  from 0.7 to 0.85 count as similar names and looser spellings are not counted.
  Navy prefixes (USS, HMS, INS, IRIS, TCG and others) imply a warship and its flag.
  One `POST /api/v1/positions` for up to 200 candidates, centred 18 h before the
  message with a 30 h tolerance (48 h before to 12 h after), gives each candidate's
  closest stored fix to the message's points (`near`, at most ten) and area
  rectangles (`near_boxes`, at most ten; zero inside), with the observation's
  `evidence_id`. Candidates with agreeing type/flag and a position within 200 km
  rank first. One candidate is `suggested` only when the message gives its
  identifier, when it is the only vessel with the name, or when it is the only
  same-named vessel near the reported places and every other same-named vessel
  was recorded elsewhere in the window (9 October). A namesake with no recorded
  position there (a KeelWatch collection gap, or no AIS) cannot be ruled out, so
  the first candidate is then not suggested and the verdict says how many such
  namesakes there are.
- `PUT /evidence/vessels/decision` `{lookup,mention,vessel_key,decision}`, where
  decision is `confirmed`, `rejected` or null (clear). Confirming one vessel for a
  mention replaces any earlier confirmation for it.
- `GET /layers/ships/{id}/track` also accepts `from` and `to` (RFC 3339, at most
  30 days apart) for the track around a message.

Added 1 October 2026:

- Each read keeps the latest match of a completed lookup in `vessel_suggestions`
  (the first candidate per mention and whether it is suggested) and display labels
  for every candidate shown in `vessel_labels`. These are presentation caches;
  decisions stay in `vessel_decisions`.
- `POST /evidence/vessels/marks` `{items:[{id,revision}]}` (at most 500) returns
  `{marks:{"id|revision":[{mention,quoted,vessel_key,name,category,flag,mmsi,imo,state}]}}`
  for feed rows: a mention's confirmed vessel, else its suggestion when that vessel
  was not rejected.
- `GET /evidence/vessels` and `GET /evidence/vessels/nearby` add `coverage`
  `{from,to,window_hours,gap_hours,gaps}` for the window from 48 h before to 12 h
  after the message (2 October). The reader says when KeelWatch was not collecting
  for an hour or more of it: a ship with no recorded position near the place may
  still have been there. Null when the message has no time or KeelWatch cannot
  answer; matching itself is unchanged.
- `GET /evidence/vessels/nearby?id&revision` lists the vessels KeelWatch recorded
  within 50 km of the message's first two points (nearest first) or inside its first
  area rectangles, from 48 h before to 12 h after the message, at most 200 per
  place, with first/last hours relative to the message. Free; it answers "which
  ships were there" for a message that names no ship.
- `GET /processing/vessels` returns `{flagged,waiting,auto,auto_since,auto_paused_reason,auto_waiting,model,totals,run}`
  (revised 9 October). Flagged messages are latest revisions from sources being read
  whose text detection says they may name ships (ship prefixes need a word boundary;
  "INS" and "IRIS" count only in capitals; detection results are cached per
  revision in `vessel_detections`). Waiting ones have no attempt that left Relic:
  attempts never sent, or refused for exhausted credits, return to the backlog.
  `POST` `{action:"start"}` runs the extraction for every waiting message (8 at a
  time, each followed by free matching in the same slot); `{action:"stop"}` starts
  no new request and lets requests already sent finish and be recorded (an attempt
  stopped between reservation and sending is recorded `not_sent`, $0). Processing
  Stop does the same (the run has no job record, like Jev and event links), and
  Pause in Processing ends a run before its next request.
  `PUT` `{auto:true}` records `auto_since` and starts nothing; every two minutes
  automatic mode reads only flagged messages whose latest revision was acquired at
  or after `auto_since` (`auto_waiting`). A run that stops early (Stop, Pause, five
  failed requests in a row, exhausted credits, a configuration error) switches
  automatic mode off and records `auto_paused_reason`; shutdown does not.
  `totals.requests`, `known_cost_usd` and `unknown_cost_requests` cover every attempt
  that left Relic, including retries and older profiles (a retry copies the
  replaced attempt into `vessel_lookup_attempts` first); a sent attempt with no
  reported cost is unknown cost, never $0. No request or spending cap. The reader's
  Try again is the explicit retry of a failed or unknown attempt.
- Evaluation gate (9 October): the extraction profile needs a passed fictional
  evaluation installed for its exact fingerprint (model, provider routing, prompt,
  schema, request parameters and `relic.vessel-decoder.v1`) under metadata
  `vessel_finder:evaluation`. Without it, Find ships, `{action:"start"}`,
  `{auto:true}` and any reservation refuse with 409 and nothing is reserved or
  billed; automatic mode switches itself off with that reason. Reading kept
  results, decisions and KeelWatch matching are not gated. `GET`/`POST
  /evidence/vessels` and `GET /processing/vessels` add `evaluated` and
  `evaluation:{passed,current,checked_at,mode,cases,fingerprint_sha256,summary}`.
  `examples/vessel-mentions-evaluation.json` holds 40 fictional cases (28 naming 34
  ships, 12 naming none) in five languages; `app/examples/evaluate_vessels.rs`
  runs them (paid, replies retained) or replays them with zero calls, and
  `activate_vessel_evaluation.rs` installs a passed record into a stopped workspace.
  Thresholds: every reply decodes with exact quotes, recall and precision ≥ 0.9, no
  ship on the 12 negatives, zero identifier errors, stated flag/type ≥ 0.8, Latin
  spellings ≥ 0.9. First run (9 October): 40/40, recall and precision 1.0,
  identifiers exact, attributes 0.97, Latin 0.92, $0.0038.
- `GET`/`POST /evidence/vessels` add `lookup.billing` (`sent`, `not_sent`,
  `rejected`), `earlier_attempts` and `identifiers_not_matched` (identifiers beyond
  the first 10 written in the message, which are not looked up). Every match is a
  full object with `error` (null when KeelWatch answered): when a lookup, search,
  positions or history request fails, the verdict starts "KeelWatch could not be
  checked", nothing is suggested, and an earlier saved suggestion is kept. A
  "Find ships" request runs in its own task, so closing the tab cannot leave it
  reserved. IMO and MMSI resolving to different vessels are not suggested; the only
  namesake recorded more than 200 km from the reported places is not suggested.

## Sea features — location context v18 (1 October; revised 9 October 2026)

`reference/marine-feature-types.json` reviews GeoNames feature types for small sea
features: straits (STRT), channels (CHN, CHNM), narrows (NRWS), shoals (SHOL),
reefs (RF, RFC) and atolls (ATOL). When such an identity is the single selected
candidate, no OpenStreetMap outline exists, and the place is an event place of a
reported occurrence, its gazetteer point is published with precision
`approximate_marine_feature` (the message precision too): approximate, not an
observed coordinate. Context mentions keep the identity without a point. A sea
feature's filing country is neutral: it neither sets the message country nor
conflicts with places in the same quote (Bab el-Mandeb is filed under Djibouti,
Mocha lies in Yemen). A supplied or written country that differs from the filing
country keeps the feature only when GeoNames lists that country among its
alternate country codes (cc2) or the point lies within about 100 km of that
country's loaded outline; otherwise the conflict stays. With no supplied, written
or message country, a feature is placed only when its exact name has no rival in
the loaded global baseline; otherwise it is `country_context_required`. Channels
(CHN), narrows (NRWS) and shoals (SHOL), which GeoNames also uses inland, need a
loaded Marine Regions area within about 25 km. "X Strait", "the X
strait", "Strait of X" and the Arabic and Persian words for strait are retried
without the word, and only a reviewed marine feature is accepted from that retry,
so "Strait of Hormuz" can never become Hormuz island. Undersea (U-class) features,
banks, gulfs and bays are not listed. The reviewed file's hash is part of the
location-context identity (`relic.location-context.v20` since 26 October 2026);
replay makes no model calls.

Foreign namesakes (v20, 26 October 2026): a small ordinary place (GeoNames
populated place that is neither a capital nor an administrative seat, under 15,000
people), given without a country and matched only because one listed place
worldwide has its name (`exact_geonames_name`), is not placed when the message's
other event places point to other countries and nothing in the message (issuers
and its established country included) points to its own. Sea features and other
such lone matches are no evidence. Status `foreign_namesake`, no point, not an
occurrence place; a claim's place follows the message's reading of the same
mention. Example: the Jordanian village of Hamoud (Karak) is not in the
gazetteer, and the one listed Hamoud is a Mauritanian village, so reports of the
Karak attack no longer land in Africa. v19 (one hour, same day) also rejected
capitals and seats abroad (Paris, Hamburg, Tampa); v20 replaced it.

## Reading choice per source — 27 September 2026

`PUT /processing/sources/{source_id}` with `{"reading": true|false}` adds or removes
one retained source from `policy.eligible_source_ids` atomically (read and written
under the writer lock) and returns `GET /processing`. The declaration record for
external processing is updated as for `PUT /processing/policy`. Removing the last
selected source also pauses reading (`enabled:false`); selecting a source never
resumes it. An unknown source is rejected. The 100-source limit on the policy was
removed.

Queries now split pending text: `unread` counts only sources selected for reading
(the reading backlog); `not_selected` counts pending text from other sources.

## MarineTraffic ships and warning windows — 26 October 2026

MarineTraffic is a live-only second ship source beside KeelWatch, chosen per
browser in Layers. The browser never contacts MarineTraffic or its image host;
every request goes through `MARINETRAFFIC_PROXY_URLS` and fails closed. Errors
follow the live-layer rules above (502 failed, 503 resting or `not_configured`).

- `GET /layers` adds a card `{id:"marinetraffic",provider:"MarineTraffic",configured,family,temporal,coverage,health}`.
  Its health is degraded only while every configured proxy rests (after a
  rate limit, or each refused by the firewall or failing); nothing is sent to
  find out. `GET /source-attention` counts it then. Not configured is quiet.
- `GET /layers/ships/marinetraffic?west&south&east&north&zoom` returns the
  ships in the view as point features with the property names the ships layer
  draws: `id` (`mt:{ship_id}`, or `mt:sat:{hash}` for a satellite contact whose
  identity MarineTraffic withholds), `provider:"marinetraffic"`, `ship_id`
  (null when withheld), `name`, `cat` (Relic ship category from the map legend),
  `type`, `type_code`, `flag`, `dest`, `sog`, `cog`, `hdg`, `rot`, `len`, `wid`,
  `dwt`, `nav` (satellite rows only), `age` (minutes from the report to
  `acquired_at`), `ts` (report time, epoch seconds), `sat`, `identity`
  (`"withheld"` or null) and `stale`. There is no feature id. The request zoom
  is the map zoom plus one, stepped down until the view needs at most nine
  tiles. `meta` has `tiles`, `requests`, `cached`, `stale_tiles`,
  `failed_tiles`, `failed_bounds`, `rows`, `rejected`, `proxy_index` and
  `coverage` ("a sample when zoomed out"); top-level `acquired_at`, `stale`
  (some tiles are the last answer, up to 10 minutes old), `incomplete` (some
  tiles failed; ships there were not received, not absent) and `error`. When
  every tile fails and none is cached the request fails.
- `GET /layers/ships/marinetraffic/{ship_id}` (`mt:` prefix allowed) returns
  `{vessel,acquired_at,stale,error}`; `vessel` uses KeelWatch's field names where
  they mean the same (`name`, `type`, `category`, `flag`, `mmsi`, `imo`,
  `callsign`, `lon`, `lat`, `observed_at`, `speed_knots`, `course`,
  `nav_status`, `destination`, `length_m`, `width_m`) plus `load_condition`,
  `ais_source`, `station`, `photos`, `particulars_known` and `particulars`
  `{last_port,next_port,departed_at,eta,voyage_progress,dwt,draught_m,max_draught_m,year_built,subtype,market,size_class,ais_class,home_port}`.
  The info window is reused for 2 minutes and served stale (labelled) for up
  to 30 minutes after a failure; particulars for an hour. A withheld satellite
  contact is 404.
- `GET /layers/ships/marinetraffic/{ship_id}/photo` returns the default photo
  (`image/webp`, `jpeg`, `png` or `avif`, `Cache-Control: private, max-age=3600`)
  or 404 when MarineTraffic has none.
- Not supported for MarineTraffic: past moments, tracks, coverage gaps, search
  and the ship finder. These stay KeelWatch's; the map says so instead of
  drawing anything.

Warnings: `GET /layers/warnings` accepts `hours` (1 to 87,600), counted back
from now or, with `at`, from that moment; `days` still works as `days × 24`
hours. With `at` only warnings registered by then are returned (before, all
retained ones were, and the browser filtered them). An invalid `hours` is 400.
The answer adds `window {hours,from,to,basis}` and `retained`, the number of
warnings Relic holds registered up to the same end, so a short window's count
is never read as all. Features stay newest first.
