# Relic — product design and recreation specification

## Current owner direction — 12 September 2026

Relic is first a map-based visibility and intelligence platform: observe incoming social messages, news/events and later vessel/aircraft/entity states. Everyday use must not require manual investigations, notebooks, assessments or briefs. Their existing durable backend records remain preserved, but they are removed from normal navigation and are not current release prerequisites. Autonomous cross-source investigation is a later capability, introduced only when useful. This owner direction supersedes the manual-investigation-first screen and delivery requirements below; Rust, storage, source/proxy, exact-evidence and responsiveness contracts remain in force.

Show a message once. Put the original, revision metadata, extracted support and provenance behind a compact message menu. No repeated generated headline plus translation, analysis-tools accordion or global graph in ordinary viewing. Provenance is a small view of direct connections, with deeper paths opened deliberately. Map controls stay left; the feed and message view stay right. Use no subagents unless the owner changes that preference.

## 1. Recommended direction

Relic is a personal intelligence workbench organized around **watching a question, examining evidence, recording a judgment, and explaining what changed**. A map is an important view of that work. Collection, datasets, processing flows, and operational health support it. The product should preserve the depth of the original concept while making the analyst's question and the evidence behind an answer easier to follow.

This specification describes the target product. It favors coherent scopes, durable analytical work, exact citations and clear limits over reproducing historical implementation quirks. The complete handoff is contained in this folder; the developer does not need access to any former application code.

The intended primary user is **one advanced analyst, with room to grow**. The first release should deliver a complete personal workflow, including meaningful evidence inspection and a portable result. Team administration, social features, enterprise approval systems, and a general integration marketplace remain outside the first release. The owner requires Rust for the backend, processing core and source adapters and has agreed to a React/TypeScript/Vite web frontend with MapLibre. `ARCHITECTURE.md` records those implementation decisions, the frontend contract and performance gates; this specification defines the product behavior. Initial storage is SQLite plus content-addressed files on a local filesystem. The owner-selected AI baseline (22 September 2026) is `openai/gpt-6-luna` through OpenRouter, with `openai/text-embedding-3-small` when embeddings are enabled; follow `CONFIGURATION.md` for routing and evaluation. Remaining schema/deployment details and internal representations are reasoned implementation choices. The supplied custom source transports are Rust modules; no Python worker is part of the architecture. Three integrations are fixed product choices: the existing custom X connector, the existing custom Flightradar24 connector, and the owner’s KeelWatch service for ships. All X and Flightradar24 traffic must pass through the configured proxy. These choices take precedence over general provider-selection recommendations throughout this document.

### 1.1 The most consequential changes

| Priority | Recommendation | Why it matters |
| --- | --- | --- |
| Foundation | Make the brief describe desired behavior, with stable requirement identifiers and acceptance evidence | A recreating agent needs to know which statements are obligations, recommendations, examples, and unresolved dependencies. |
| Foundation | Use one explicit analysis scope across map, list, counts, assistant, watches, and briefs | Otherwise two apparently identical views can answer different questions without the analyst noticing. |
| Foundation | Separate evidence, claims, incident grouping, analyst assessment, and work status | Repetition, location confidence, corroboration, and confirmation cannot safely share one status or score. |
| Foundation | Preserve record identity through correction, backfill, merge, and split | Notes, citations, watches, and saved work should survive an improving interpretation. |
| Foundation | Distinguish when something happened from when the workspace learned it | A later correction should not silently alter a reconstruction of an earlier decision. |
| Foundation | Give every integration a capability, coverage, retention, and usage profile | A provider name does not establish complete history, permission for AI processing, or unlimited local storage. |
| First release | Organize the main workflow around Watch, Investigate, and Library; place operational detail in Workbench | Preserve analyst power while making everyday work independent of system maintenance. |
| First release | Add saved watches, an actionable inbox, and saved investigation briefs | The product should help the analyst complete an investigation, not only inspect transient panels. |
| First release | Keep all evidence searchable, including reports without coordinates and material excluded from an event view | Extraction or mapping failure should not make a potentially important report disappear. |
| First release | Put AI quality and exact citations ahead of serving speed | Fast unsupported output is a poor analytical result. Cost and latency remain visible constraints. |
| First release | Add import, export, recovery, and usable empty/demo states | A standalone workbench should be useful without every external service and should not trap the analyst's work. |
| Later | Introduce more sensors, automatic hypotheses, richer graph exploration, and collaboration only when their contribution can be demonstrated | Breadth creates operating cost and additional uncertainty; it is not automatically better intelligence. |

### 1.2 What remains valuable

Keep source-preserving acquisition, versioned analytical outputs, inspected transformations, optional AI, scoped geographic correction, map uncertainty, colocated-event access, linked report/media evidence, source-aware movement history, provider health, and bounded work. Keep direct expert controls and a meaningful processing graph. Keep the distinction among collected reports, live observations, reference geography, and derived analysis.

Change their organization and contracts. A clean interface should not conceal evidence, and an advanced interface should not require constant interaction with maintenance gauges. The application should retain strong defaults while allowing the analyst to inspect and deliberately override them.

### 1.3 How this brief is binding

This is a **recommended target specification**, not a statement that every recommendation has been validated with users or implemented. Requirement identifiers define the proposed behavior. The release table defines when it is needed. Design defaults are explicit hypotheses that can be changed with a recorded rationale and supporting evaluation. Provider facts are dated dependencies to recheck before integration.

A statement marked **Must** defines behavior required for its assigned release. **Should** is the recommended design, with a documented alternative permitted. **Explore** identifies an experiment and must not be silently included in the first-release build. No requirement in a later phase is silently promoted to first-release scope merely because it appears in a detailed catalogue.

The separation between requirements, assumptions, rationale, and verification follows established requirements-engineering guidance. NASA's checklist emphasizes implementation-independent, unambiguous, individually referenceable requirements with realistic verification criteria. This brief applies that principle to a personal software product; it does not import aerospace process overhead.[^1]

## 2. Product outcomes and working assumptions

### 2.1 The analyst's jobs

**Watch:** define a question, geography or entities, sources, time horizon, and conditions that justify attention. Open the workspace and understand meaningful changes without rereading the entire archive.

**Investigate:** move from a signal to source material; compare supporting, contradictory, and repeated reports; inspect geography and time; record uncertainty and a reasoned assessment.

**Explain:** create a brief whose statements link to evidence and whose scope, cutoff, limitations, and revisions are clear. Reopen it later and understand what information was available when it was written.

**Maintain:** repair missing coverage, failed collection, poor enrichment, and ambiguous geography without losing the investigation context. Inspect costs and recovery only to the level needed to decide what to do next.

**Experiment:** change a processing recipe, compare results, and decide whether the change improves the analyst's evidence. An experiment should not unexpectedly change active watches or saved judgments.

### 2.2 Recommended product shape

Use five primary destinations: **Watch**, **Investigate**, **Library**, **Sources**, and **Workbench**. Settings, access, backup, and service configuration belong in a compact workspace menu. This is a proposed information architecture; the required distinctions and journeys matter more than the exact names or page count.

| Destination | Main question | Core components |
| --- | --- | --- |
| Watch | What changed that deserves attention? | Saved-watch selector, change digest, actionable inbox, coverage warnings, focused map/list, quiet-state summary. |
| Investigate | What does the available evidence support? | Shared scope bar, map/list/timeline views, entity/event detail, evidence inspector, claim comparison, notes, assistant, saved brief. |
| Library | What has been collected, derived, and saved? | Global search, reports/media, incidents/entities, datasets/versions, saved investigations/briefs, imports/exports. |
| Sources | What can I collect, what did I actually collect, and where are the gaps? | Source-type catalogue, setup and test, acquisition controls, coverage/usage profiles, run history and recovery. |
| Workbench | Why is the system producing this result, and how can I improve it? | Processing recipes and expert graph, output comparison, lineage, quality review, geographic references/corrections, service health and budgets. |

The analyst may pin Workbench or start on a saved investigation. The default startup should resume an active watch or show a concise selection of saved watches. A new workspace should offer an explainable demonstration dataset and a first-source/import path, rather than an empty operational graph.

### 2.3 Deliberate alternatives

| Choice | Recommended | Credible alternative | Tradeoff |
| --- | --- | --- | --- |
| Main workspace | Watch-led with immediate investigation | Map-led with watch inbox docked beside it | Watch-led prioritizes change; map-led is better for continuous spatial patrol. Both need global search and unlocated evidence. |
| Information architecture | Five task-oriented destinations | Retain seven destinations but start in Map/Watch and demote technical tabs | Five reduces navigation overhead; seven may be more familiar to the original analyst. Validate with real task walkthroughs. |
| Processing authoring | Templates and parameterized recipes plus an expert graph | Graph-first for every workflow | Recipes shorten routine setup; the graph preserves composability. One must not become an inconsistent second implementation of the other. |
| Evidence reasoning | Claim and dependency records with an optional graph view | An always-visible knowledge graph | Explicit records improve provenance immediately. A permanent graph can cost attention before it improves an investigation. |
| First-source strategy | The selected custom X/Flightradar24/KeelWatch integrations, staged around a complete workflow, plus analyst-owned/permissioned imports | Add unrelated source families after the core workflow | The three selected integrations remain fixed; staging their interfaces does not authorize replacing their acquisition paths. |
| Time exploration | Event-time and known-at-the-time modes | Event-time only, with immutable saved briefs | Full replay is more demanding. If deferred, label the limitation and preserve saved brief evidence; do not simulate complete replay. |
| AI autonomy | Automatic reversible analysis; controlled durable promotion | Manual approval for every model result or unrestricted automatic correction | Both extremes damage usefulness. Separate suggestions from judgments and use bounded batch policies. |

Progressive disclosure is an appropriate design pattern for placing expert controls behind a clear entry point while retaining frequent actions on the main surface. It does not justify hiding active filters, limitations, or errors.[^2]

## 3. Shared product contracts

### 3.1 One analysis scope

**SCOPE-01 — Must, first release.** Every analytical surface uses an explicit scope describing: the selected collection or dataset versions; time interval and time basis; geography; selected/excluded source instances; query terms; event/claim categories; relevance and review filters; duplicate treatment; and rules for including uncertain or unlocated evidence. Give each applied scope a stable revision identity.

**SCOPE-02 — Must, first release.** Distinguish a fixed area of interest from the current camera viewport. Panning changes the camera by default. When “search this view” is active, camera changes deliberately update geographic scope and are visibly marked. A briefing generated for an area must not change its meaning because the camera was moved afterward.

**SCOPE-03 — Must, first release.** Map, list, statistics, situation brief, assistant retrieval, and watch preview return the same eligible evidence set when they claim the same scope and cutoff. Pagination, grouping, and visual aggregation can differ; membership and totals cannot silently differ. Display the base total, loaded count, included unlocated count, and exclusions when material. Scoped support counts and generated summaries use eligible evidence only, including when an incident was originally assembled from a wider set. A preserved assessment based on a wider scope may remain visible with that original scope labeled; it must not appear to have been derived from the current scope.

**SCOPE-04 — Must, first release.** If an assistant or auxiliary analysis broadens or narrows the scope, show the resulting scope and reason alongside its answer. An explicit user request to search more widely can perform that search directly. It must not masquerade as an answer about the original visible set.

**SCOPE-05 — Must, first release.** Saving a view stores analysis scope separately from presentation state. Shareable links or exported context must identify whether they point to a moving latest view or a pinned snapshot. A missing source/version/permission yields a specific unavailable state rather than quietly changing the query.

### 3.2 Evidence and interpretation

**EVID-01 — Must, foundation.** Preserve distinctions among an acquired evidence item, an individual report or observation, a claim extracted from it, a real-world entity hypothesis, an incident grouping, an analytical assessment, and an analyst's work status. A single row may link these objects but must not collapse their meaning.

**EVID-02 — Must, foundation.** Every derived assertion links to the exact evidence revision and relevant text span, media region/time interval, or measured observation that supports it. Preserve original language and units beside translations/conversions. A citation to a nearby report or a later revision is not an exact citation.

**EVID-03 — Must, foundation.** Keep source count, origin/dependency count, evidence modality, unresolved dependence, contradictory evidence, and analyst assessment distinct. Reposts, copied articles, common press releases, syndicated feeds, and quoted material are candidate shared origins. Unknown dependence stays unknown. No fixed source-count threshold automatically establishes truth.

**EVID-04 — Must, first release.** A claim can be asserted, denied, corrected, withdrawn, or unresolved in the source material. Analyst assessment can be supported, contested, insufficiently supported, or not assessed, with rationale. Work status can be new, in review, resolved, snoozed, or irrelevant to a particular watch. Changing one dimension must not silently change the others.

**EVID-05 — Must, first release.** A model-generated or analyst-entered judgment identifies who/what produced it, when, which evidence was considered, and why. Confidence labels name the dimension they describe. Geocoding fit, similarity, model response confidence, source reliability, and claim probability are not interchangeable values.

**EVID-06 — Must, foundation.** Record upstream state separately from local availability and retention state. Upstream material can be present, revised, removed, or inaccessible while a permitted retained original remains locally available. Local states include available, reference-only, policy-expired, user-purged, or never acquired. Show combinations such as “upstream removed; retained original available.” After local removal, retain only permitted metadata and explain which historical views, citations, and recalculations are no longer reproducible.

### 3.3 Time, revision, and identity

**TIME-01 — Must, foundation.** Preserve event time or interval, publication time, source revision time when supplied, first acquisition time, observation/fetch time for sensors, processing time, and analyst-decision time. Unknown timezone, estimated timestamp, and uncertain interval are explicit. Use one declared internal comparison convention while allowing a chosen display timezone.

**TIME-02 — Must, first release.** Event-time exploration answers what is reported to have happened in a period using a selected retained snapshot or the current complete publication. Saved briefs pin that snapshot, its cutoff and the interpretation revisions used. First-release Replay filters event time within that selected result; it does not promise reconstruction at any arbitrary historical knowledge cutoff. Late-arriving old evidence appears as newly learned information without being relabeled as a newly occurring event.

**TIME-03 — Must, investigation release.** A known-at-the-time view answers what the workspace had acquired and derived by a specified knowledge cutoff. It excludes later evidence and later judgments unless the analyst requests a separately labeled retrospective reconstruction. Replay coverage must disclose which external layers and expiring evidence can be reproduced.

**ID-01 — Must, foundation.** Evidence identities survive repeated acquisition and content revisions. Entity and incident identities survive ordinary updates and retain an explicit history of merges, splits, aliases, and supersession. Keep a link from any prior identity to its current disposition.

**ID-02 — Must, first release.** After a split, do not copy a whole-incident assessment to every successor. Preserve the original assessment and ask where its evidence applies through an explicit reassessment item. Evidence-level notes remain attached to the original evidence. After a merge, retain both prior histories and identify conflicting assessments.

**ID-03 — Must, foundation.** Provider identifiers are qualified by their source namespace. A name match, spatial proximity, inferred flag, or recycled registration is not enough to silently merge physical entities. Suggested matches retain their evidence and can be accepted or rejected reversibly.

### 3.4 Incomplete and degraded operation

**STATE-01 — Must, foundation.** Separate no observations, unavailable service, incomplete response, unsupported query, delayed collection, paused collection, expired evidence, and filtered-out results. An empty view carries its reason and a relevant recovery action.

**STATE-02 — Must, first release.** Keep the last usable result visible during a refresh when appropriate, with its data time and refresh state. A failed refresh must not relabel cached data as current. Failures in auxiliary enrichment must not remove original evidence or disable unrelated analysis.

**STATE-03 — Must, first release.** Preserve unsaved analyst work during navigation, recoverable network interruption, and reauthentication. Persistent failures belong in an activity/problem area; transient toast messages alone are insufficient for failed saves, delivery problems, or lost coverage.

**STATE-04 — Must, foundation.** All irreversible or externally consequential actions identify their effect before commitment. Routine reversible view actions execute directly. Support undo/history for edits and scoped batch operations. The operator can preauthorize bounded recurring behavior rather than approve each repeated item.
## 4. Watch and attention workflow

### 4.1 Saved watches

**WATCH-01 — Must, first release.** A watch stores a title, analytical question, area/entities/topics of interest, shared scope, selected sources, attention conditions, collection/AI budgets, and active/paused state. Examples include monitoring a port disruption, tracking developments around an incident, or detecting new contradictory evidence for a saved assessment. A watch may concern non-geographic material.

**WATCH-02 — Must, first release.** Creating a watch offers a readable condition builder with source, time, geography, categories, entities, support state, and material-change conditions. Expert expressions remain available and show their equivalent scope where expressible. A rule whose expression cannot be translated into the visual editor stays editable as an expert rule; opening it must not lose meaning.

**WATCH-03 — Must, first release.** Before activation, show a dry run over retained representative history: matched items, reasons, grouped alert count, estimated frequency, coverage limits, and cost estimate or unknown-cost state. Testing does not deliver notifications or advance the active rule's evaluation cursor. Keep rule revisions and explain whether activating a revision uses a fresh baseline or a bounded retrospective review. With unavailable or insufficient history, show frequency unknown and allow activation with a clearly declared fresh baseline. An empty or unrepresentative sample must not be presented as proof of zero future matches.

**WATCH-04 — Must, first release.** The watch header summarizes available coverage and delayed/failed dependencies. A quiet period while two important sources are offline is different from a quiet period with expected collection healthy. An acquisition failure should create a coverage problem, not an intelligence conclusion that nothing occurred.

### 4.2 An actionable inbox

**ALERT-01 — Must, first release.** An alert identifies the watch/rule revision, triggering evidence, evaluation scope/cutoff, reason, first/latest occurrence, material changes, and delivery state. Selecting it opens exact evidence even if it is outside the currently loaded map sample.

**ALERT-02 — Must, first release.** Provide acknowledge/review, snooze with expiry, dismiss-as-irrelevant with optional reason, open investigation, and pause this watch. These actions affect attention and workflow; they do not declare the underlying claim true or false. Acknowledgement and successful message delivery are separate states.

**ALERT-03 — Must, first release.** Group repeated matches by a stable analytical identity and explain the group. Notify again for configured meaningful changes, such as a new observed origin, contradiction, material geographic change, new measurement, or reopened assessment. Simple reprocessing, renaming a node, or detecting another copy of the same report should not independently create a fresh alert unless requested.

**ALERT-04 — Must, first release.** Support quiet hours, digest delivery, urgent overrides chosen by the analyst, and a maximum interruption budget. Grouping and suppression retain inspectable counts and reasons. A digest must not hide a correction to an earlier urgent alert. Exact default limits are calibrated through the acceptance fixture rather than asserted as universal human limits.

**ALERT-05 — Must, foundation.** Persist a matched event and its delivery obligation coherently. Retrying a delivery uses the same logical alert identity. Show pending, delivered, failed, and exhausted-retry states. Delivery channels expose their actual acknowledgement guarantees; the app must not promise exactly-once external delivery without evidence.

**ALERT-06 — Must, first release.** First activation establishes a baseline by default and labels that choice. An optional backfill review places old matches into a separate review set without pretending they are newly occurring. Loss of an old comparison version creates a declared rebaseline/review event rather than a silent flood of historical alerts.

### 4.3 Change summaries and attention ranking

**CHANGE-01 — Must, first release.** “Since I last looked” reports the prior cutoff, current cutoff, new evidence, materially updated incidents/claims, retractions, changed assessments, and relevant coverage changes. Merely opening a page does not consume unread changes. Mark seen is explicit or happens through a documented reading action. Keep a way to revisit the previous digest.

**CHANGE-02 — Must, first release.** Show separate default views for chronological changes and suggested attention. Explain every attention rank using interpretable factors such as the watch match, novelty relative to known origins, a contradiction, geographic uncertainty, or a changed operational condition. Popularity elsewhere on the internet is not a default importance signal.

**CHANGE-03 — Must, investigation release.** Activity comparisons include the current and baseline scopes, bucket size, source participation, collection continuity, origin/repetition treatment, and amount of usable baseline. A changed source portfolio, outage recovery, or backfill can invalidate a rate comparison and must be flagged. An unobserved baseline is not a measured zero.

**CHANGE-04 — Should, investigation release.** Compare similar local time/day patterns when sufficient history exists and let the analyst inspect the raw series. Show effect size and supporting counts before any significance label. Multiple simultaneous tests and thin samples limit interpretation; a generic “threat score” is out of scope. Keep simple count differences if a defensible baseline is unavailable.

## 5. Investigations, evidence reading, and briefs

### 5.1 A durable investigation

**INV-01 — Must, first release.** An investigation stores its question, current scope, evidence selections, candidate incident/entity relationships, claims under examination, notes, assumptions, alternatives, assessment revisions, and unresolved questions. It can start from a watch alert, any evidence item, search results, a map selection, or an empty question.

**INV-02 — Must, first release.** Pinning evidence records the exact retained revision and the analyst's reason for including it. It does not copy the item into a new independent source. Evidence can belong to more than one investigation. Removing it from one investigation leaves the acquisition record and other investigations intact.

**INV-03 — Must, first release.** The detail workspace has clear sections for member evidence, related context, claims/assessments, and changes. Open explicit incident members from their references. Nearby reports and live sensors appear as context with their relation, distance, and time; proximity alone must not add them to membership or support counts.

**INV-04 — Must, first release.** The analyst can attach an overlooked report, compare two reports, suggest a merge or split, correct an extracted claim, and state why evidence is irrelevant. Preserve the original extraction and show the analyst's amendment separately. Consequential relationship changes show affected watches, assessments, and briefs before applying.

**INV-05 — Should, first release.** Offer a lightweight alternatives section: current explanation, plausible alternative, evidence that would distinguish them, and unresolved gap. This should be optional for routine triage and easy to expand for a substantial assessment. Do not require formal hypothesis scoring for every event.

Analytic standards provide a useful precedent for distinguishing source information, assumptions, judgments, uncertainty, alternatives, and changed judgments. Those distinctions inform this product; they do not imply certification or that a particular interface is mandated.[^3]

### 5.2 The evidence inspector

**READ-01 — Must, first release.** The default evidence card shows title/identity, issuer or source, relevant times, evidence family, available original, revision/deletion state, geographic precision if any, and why it is in the current scope. Secondary fields expand in place. Exact source references must be reachable without inspecting a technical run log.

**READ-02 — Must, first release.** Support original and translated text side by side or a clear switch that preserves reading position. Highlight the passage cited by a claim. For supported imported documents, show the page/paragraph reference. When audio/video inspection is introduced, support retained timestamps and the relationship of transcript to original; this does not require advanced audio/video processing in the first release. Unsupported extraction leaves the original usable with a legible limitation.

**READ-03 — Must, first release.** Text search includes originals and derived text, with language/transliteration alternatives where supported. Show which field matched. Provide a raw-evidence lane for denials, retractions, future plans, historical material, and unresolved classifications. An event filter can hide them from event dots without removing them from search or an investigation.

**READ-04 — Must, first release.** Media inspection distinguishes original retained asset, display rendition, source caption, any extracted text, claimed capture time/location, local reuse matches, and analyst annotation. A match links to the earlier locally observed item and names the comparison type. Similarity does not establish authorship or original publication time.

**READ-05 — Explore, later.** Optional provenance-credential inspection can report available, absent, invalid, unsupported, or validated credentials under a stated policy. Valid provenance is not a true-scene/true-caption verdict; missing provenance is not a fake verdict. Automated authenticity or damage scores require separate evaluation and must remain attributed indicators.[^4]

### 5.3 Saved briefs and portable outcomes

**BRIEF-01 — Must, first release.** The analyst can create and save a readable brief from an investigation. It includes the question, scope and cutoff, concise findings, supporting/contradicting evidence, analyst assessment, assumptions, gaps, and source coverage. Every factual evidence-based statement must have inspectable support or be explicitly presented as an unresolved or unsupported proposition.

**BRIEF-02 — Must, first release.** A generated draft is editable. Distinguish authored changes from generated content, retain the original draft for review, and save a new brief revision when updated. Refreshing evidence does not rewrite a previously saved brief. Show when new information may materially affect it.

**BRIEF-03 — Must, first release.** Export a human-readable brief plus a structured manifest of scope, retained evidence identities/revisions, provenance, and omissions. Include original content/media only where permitted and selected. Make redactions and expired/missing support explicit. The exported report must be understandable outside the app; a live link alone is not a portable outcome.

**BRIEF-04 — Must, first release.** A brief may include a map or chart as an illustration, with its legend, units, time mode and scope. It must remain readable through a textual/table alternative. Avoid printing unexplained glyph colors or live-relative phrases such as “five minutes ago” into a frozen report.

**BRIEF-05 — Should, investigation release.** Compare brief revisions by changed findings, new evidence, removed/expired support, and changed assessments. This is more useful than a raw text diff alone, although the exact text diff can remain available.

The recommended provenance model borrows the distinctions among entities, activities, responsible agents, revisions, and derivations from W3C PROV. These are conceptual requirements; they do not prescribe a graph database or serialization format.[^5]

## 6. Library, search, datasets, and portability

**LIB-01 — Must, first release.** Provide a searchable catalogue of reports/observations, media, incidents/entities, datasets, and saved investigations/briefs. Filters use the shared scope contract. Non-geographic evidence has a full reading path. A search result can open independently of the map's current dataset or capped sample.

**LIB-02 — Must, first release.** The dataset detail offers current and retained historical versions, schema, source/producer, row and logical-evidence counts, origin/lineage, publication time, last verified-no-change time, and retention/coverage limits. Choosing a historical version changes the preview explicitly; it is not merely a metadata row.

**LIB-03 — Must, first release.** Preview results are bounded with clear totals or unknown total, sort order, pagination, and truncation state. Structured nested data remains inspectable. Distinguish a row count from a report count when transformations expand one report into several locations.

**LIB-04 — Must, first release.** Support bounded read-only analytical queries and saved queries with documented dataset/field semantics. A failed query leaves the prior result available and marked as such. Explain missing versions, type errors, timeouts and resource limits without exposing secrets. The query language and underlying implementation are not prescribed here.

**LIB-05 — Must, investigation release.** Version comparison distinguishes newly acquired evidence, source revision, extraction/correction changes, group membership changes, disappeared-from-current-selection items, and actual removed/expired evidence. A hash difference alone is not a sufficient analyst explanation.

**LIB-06 — Must, first release.** Allow export of an eligible selected dataset or result with schema, units, timestamps, provenance and coverage metadata. A source-specific export restriction can omit or deny affected material while explaining the result. Do not silently export an apparently complete table with undisclosed omissions.

**LIB-07 — Must, first release.** Provide a backup/restore workflow for eligible evidence, workspace configuration, investigations, saved judgments, and processing definitions. Show backup time, included scope, exclusions, and last successful restore verification. Restoring to a test workspace must reproduce a chosen saved investigation and its retained references; a completed backup job alone is insufficient evidence of recoverability.

**LIB-08 — Must, first release.** Separate retiring source configuration, removing an item from a collection, deleting local evidence, and retention expiry. Show dependencies before an explicit destructive purge. Some material may require expiry even while an investigation remains saved; preserve only permitted residual references and disclose the loss.

## 7. Source catalogue and collection design

### 7.1 A source type describes evidence, not a vendor

**SRC-01 — Must, foundation.** Model evidence family, acquisition method, provider, source instance, and workspace-use mode independently. Evidence families are authored reporting, institutional advisories, physical observations, imagery acquisitions, geographic/reference records, and analyst contributions. Acquisition may stream, poll, search a catalogue, import a file, or capture an individual item. Workspace use may browse, monitor, or preserve where supported.

**SRC-02 — Must, first release.** A source card explains what is provided, by whom, where/when it is available, update/retention behavior, supported operations, normal freshness, gaps, permitted uses and known cost constraints. The default card is concise; detailed capability metadata is supplied by the integration rather than a form the analyst must fill on every use.

**SRC-03 — Must, first release.** Distinguish integration installed, service configured, authenticated, entitled to this operation, enabled, rate-limited, healthy, stale, and failed. Test setup with a bounded sample and show what was actually returned. Secrets are entered through a dedicated concealed control and are not placed in map links, exported evidence or ordinary logs.

**SRC-04 — Must, foundation.** Collection, retention, AI processing, external-model transmission, and export have separate eligibility states based on the applicable source policy/agreement. Record the policy reference and check date. Authentication or public visibility alone does not imply every use is eligible. Unknown eligibility pauses only the affected operation; it does not disable unrelated evidence work.

**SRC-05 — Must, foundation.** Preserve original source identity and upstream origin relationships where available. Two vendors carrying one originating advisory or one sensor observation remain two acquisition paths for potentially the same evidence, not automatically two independent origins.

### 7.2 Proposed source portfolio

| Evidence type | Baseline behavior | Recommendation and alternatives |
| --- | --- | --- |
| Supplied text and notes | Preserve original input and attribution/claimed time; clearly identify analyst-authored content | First-release core. Works without external credentials. Notes are interpretation unless explicitly supplied as acquired evidence. |
| Supplied documents and media | Preserve eligible originals, source/capture information, pages or media intervals; extract text/metadata with quality flags | First-release import path for text documents, images and structured observations; fuller audio/video processing can follow. Do not require publishing material to a social network first. |
| Tabular and geographic observations | Map fields, coordinate system, units and time columns through a preview; validate rows and report rejects | First-release core. Already structured measurements bypass language-model extraction. Geographic collections can be reference or evidence depending on meaning. |
| Public reporting feeds and institutional bulletins | Poll permitted feeds/queries, retain issuer and original link, excerpt/full-text distinction, revisions and coverage | Additional adapter after the minimum X/imports/one-advisory workflow. RSS/Atom is an acquisition format, not proof of permission for full-text storage or AI. |
| Structured institutional advisories | Preserve issuer, identifier/episode, effective interval, urgency/severity/certainty, affected geometry, update/cancel links | First-release contract with one supported integration. CAP-compatible material is a useful interoperability example; the exact transport is open. |
| Telegram channels, groups and supergroups | Accessible authored reports with reply/forward context, media/revision capabilities where permitted | Optional adapter with explicit eligibility. Conversation size/type does not create a different evidence family. Avoid depending on a particular session workaround. |
| X user timelines | Use the existing custom authenticated mobile GraphQL connector: resolve handles, poll timelines, catch up through bottom cursors and preserve post/media references | Required selected integration. Retain validated token replacement and session-pinned proxy egress; every related request uses the proxy. See §7.4. |
| Ships: KeelWatch | Live viewport snapshots, canonical vessel identities, details, historical observations, nearby context, analytics and registered regional captures | KeelWatch is the selected source and owns collected maritime history. Relic analyzes and preserves qualifying snapshots; it does not replace KeelWatch collection. See §7.4. |
| Aircraft: custom Flightradar24 connector | Existing browser-feed integration: live viewport, selected-flight details, FollowFlight trail and supported enrichment | Required selected integration, through the proxy for every request. A selected-flight trail and a historical snapshot of all aircraft are separate capabilities; the latter is not assumed. See §7.4. |
| Earthquake observations | Occurrence/update time, magnitude/type, depth, location, review state and issuer link | Recommended contextual integration. Preserve revised/superseded event relationships and service completeness. |
| Thermal/fire observations | Sensor/product, acquisition time, measured power/confidence, quality and processing revision | Recommended contextual integration. A hotspot is neither a verified fire perimeter nor an attack attribution. |
| Optical satellite imagery | Acquisition identity/footprint/time, product/resolution and quality, comparison and citation | Investigation release. Treat imagery as dated evidence; ordinary basemap imagery is separate context. |
| Radar imagery or derived environmental products | Product-specific metadata, comparable acquisition conditions and interpretive limits | Explore after a demonstrated task. Do not turn a difference image into an automatic damage verdict. |
| Geographic names, boundaries and facilities | Names/aliases, geometry, type, provenance, vintage and precision | Foundation. Global coverage and sensitive facility classes are not assumed; let the analyst add justified reference packs. |
| Weather/ocean conditions, infrastructure disruption notices and official registries | Relevant observations/records with validity periods, identity and update semantics | Later, driven by specific investigations. Useful for alternative explanations; no indiscriminate all-layer world dashboard. |

### 7.3 Setup and ongoing collection

**SRC-06 — Must, first release.** Source setup chooses an eligible operation and scope, previews a bounded response, exposes expected update/coverage/cost, and offers Activate. Detect duplicate source instances by stable upstream identity. Rename, pause/resume, and retire source configuration without deleting retained evidence. State what a pause stops and what remains available.

**SRC-07 — Must, foundation.** Backfill uses explicit time/row bounds and a resumable checkpoint when the provider supports it. Advance progress only after eligible acquired data is durable. If replay is unavailable, show the unrecoverable acquisition gap rather than claiming catch-up. Repeated delivery of the same revision does not create duplicate evidence.

**SRC-08 — Must, foundation.** Treat a complete empty snapshot as a legitimate observation of the provider's qualifying population. Treat timeout, aggregate-only response, partial pagination, unsupported period, and malformed response as different outcomes. An unsuccessful request does not replace the last complete snapshot with an empty one.

**SRC-09 — Must, first release.** Show completeness separately for returned provider pages, acquisition continuity, physical/spatial observation coverage, and identity reconciliation. Unknown world coverage remains unknown even when every result page was retrieved. A vessel disappearing from a selection does not prove a departure at that moment.

**SRC-10 — Must, first release.** Choose collection cadence and geographies against the analyst's freshness needs, provider limits and budget. Enlarging a region, backfilling history or enabling more frequent paid queries shows estimated/ranged/unknown cost and respects hard limits. Camera motion alone must not create unbounded paid collection.

**SRC-11 — Must, first release.** A revised or withdrawn advisory/sensor product updates current applicability while preserving permitted previous revisions and relationships. Processing the later preferred observation must not double-count one physical event simply because its provider product changed.

### 7.4 Selected custom integrations and mandatory proxy routing

These are fixed Relic requirements. The standalone Rust clients in `integrations/rust/` preserve the custom acquisition approaches. The source-specific guides remain in `integrations/x/README.md` and `integrations/flightradar/README.md`; `ARCHITECTURE.md` records the agreed Rust backend, frontend and storage decisions. `SOURCE_INTERFACES.md` supplies endpoint and response contracts for the other sources; `CONFIGURATION.md` describes private setup. The examples contain no personal credentials or configured proxy addresses; public client identity values are labeled as protocol compatibility data.

**NET-01 — Must, foundation.** All X and Flightradar24 upstream traffic must traverse the configured proxy: session establishment, credential validation, identity lookup, timeline/feed reads, pagination, detail/trail streaming, media/images/thumbnails, auxiliary requests, protocol-catalogue maintenance and diagnostics. Resolve upstream destinations through the proxy path. Empty, invalid or exhausted proxy configuration fails closed; no direct fallback, environment bypass or silent alternate integration is permitted. A proxy failure leaves retained data usable and visibly stale.

**NET-02 — Must, foundation.** The Relic client loads X and Flightradar24 content from Relic-controlled endpoints/cache. It must not fetch upstream images, videos, embeds or source previews directly, nor initiate upstream preconnect/prefetch. Asset redirects stay on the enforced proxy path and cannot forward account credentials to unrelated hosts. Source opening uses a proxy-enforced viewer/browser; if none is available, offer a copyable reference instead of opening an unprotected network path.

**NET-03 — Must, first release.** Every network path for these integrations uses the same mandatory egress policy, including retries, streaming, media and maintenance tooling. X sessions remain pinned to one proxy; Flightradar24 may fail over only among configured proxies. Logs expose connector/proxy status without printing tokens, cookies or proxy credentials. Verification must include a missing-proxy case, all-proxies-failed case, redirect/media case and browser-network inspection. The supplied transports demonstrate the pattern; application UI and maintenance paths must pass the same checks after implementation.

**X-01 — Must when X is introduced.** Reuse the existing custom mobile GraphQL path. A configured account `auth_token` and selected proxy establish a session; a proxied visit to `https://x.com/` obtains the `ct0` cookie when it is not already supplied. Subsequent calls to `https://api.x.com/graphql/{query_id}/{operation}` carry the existing client identity, app bearer, CSRF header and session cookies. Use the supplied operation catalogue for `UserByScreenName` and `UserTweets`. It also preserves `TweetResultByRestId` as an unimplemented compatibility lead; do not assume an exact-post request contract. Reopening evidence uses its retained local revision. Operation IDs/features are compatibility data that can change; their maintenance also obeys NET-01. Keep personal session and proxy credentials only in private runtime configuration, never public documentation, source code or exports.

**X-02 — Must when X is introduced.** Resolve handles to stable user identities before monitoring. Validate a replacement token through the proxy before activating it; a failed validation preserves the working session. The existing hot-swap is in-memory and reverts to configured credentials at restart unless Relic deliberately adds explicit persistence. A poll sweep uses a session pinned to one proxy. Optional pool rotation happens between sweeps by constructing and authenticating a new client, never halfway through one session.

**X-03 — Must when X is introduced.** Poll enabled timelines and catch up using bottom cursors until the exact previous post-ID watermark is reached in an ordinary unpinned entry or the timeline ends. Old pinned posts and conversation replies do not establish watermark overlap; a page with unreadable items and a continuation cursor must continue. Requested count is a hint, not a guaranteed returned count. Deduplicate repeated/pinned posts, preserve long-form text and reply/repost/quote/media relationships, and save raw/normalized evidence before advancing the watermark. A page-budget exhaustion reports incomplete catch-up and keeps the prior watermark. Pause ordinary polling for that source and retain a durable continuation cursor, original watermark, account/source identity and safely acquired pages. Offer bounded continuation or explicit restart; never repeat the same exhausted first-page sweep indefinitely. A deliberate new baseline discloses the abandoned gap. Missing ordinary checkpoints stop at the first older own ordinary entry without declaring coverage complete; retain raw pages and the old checkpoint. Pause after 30 pages until explicit continuation. Sources exposes an atomic restart of recent history that records the abandoned interval and preserves acquired evidence. The X reference README defines the continuation contract. The present connector reads posts; it does not establish complete historical edit/deletion reconciliation or a general search/stream capability. Expose those limits honestly. Media URLs are references until retrieved through Relic’s controlled proxy path.

**FLIGHT-01 — Must when aircraft are introduced.** Reuse the existing custom Flightradar24 browser-feed client. The current implementation sends framed protocol messages to `https://data-feed.flightradar24.com/fr24.feed.api.v1.Feed/{method}` with the existing browser identity headers, entirely through the proxy pool. `LiveFeed` provides viewport aircraft; `FollowFlight` provides selected-flight details and trail. The inspected feed uses identity headers without individual feed credentials. Keep protocol behavior distinct from workspace access control and do not invent extra account setup.

**FLIGHT-02 — Must when aircraft are introduced.** Retain upstream category filtering, bounded viewport subdivision and deduplication by flight identity. A full tile can mean truncation; reaching the request/depth bound must expose incomplete coverage. Decode unary frames/trailers strictly and check protocol status even after a successful HTTP response. Decode streamed base64/frame boundaries incrementally, preserve distinct position versus follow-message schemas, and stop/close the stream when enough data has been acquired. A selected-flight trail does not establish replay of the whole historical aircraft population.

**FLIGHT-03 — Must when aircraft are introduced.** Preserve current position, observation times, callsign, available airframe/route fields and independent loading of detail/trail. Keep barometric altitude in feet and ground speed in knots identified before display conversion. The position-buffer deltas are earlier fixes, not predictions. Supported airport/clock/other enrichments use the same proxy transport; aircraft photographs use a Relic-controlled asset endpoint. No optional enrichment may open a direct route when its proxy fails.

**KW-01 — Must when ships are introduced.** Use the owner’s KeelWatch service for live viewport observations, vessel details, collected historical observations, nearby queries, movement analytics and registered regional snapshots. Configure the service URL and its bearer token where enabled. KeelWatch owns maritime collection/history; Relic’s map journal is only a sampled viewing record. Canonical vessel IDs are strings, with MMSI/IMO as separate assertions and confirmed aliases preserving old references. Keep observation, reception and acquisition clocks distinct.

**KW-02 — Must when ships are introduced.** Use complete conditional viewport/nearby responses, with an exact-request cache scoped to service and credential identity; a 304 requires that exact cached response. A complete empty response clears current membership, while failure retains labeled stale evidence. Rendering grid cells are aggregates, never historical observations. Regional captures run while the map is closed and reject partial/truncated/aggregate snapshots, preserving the previous complete publication. Identical captures update verification without duplicating versions; a newly complete empty capture publishes an empty current population. Query vessel history directly from KeelWatch with the requested time interval and show gaps/truncation. Malformed required observation fields or aggregate rows block replacement of a complete regional version. Preserve eligible raw data for diagnosis and show rejected counts; do not publish a silently reduced snapshot as complete.

### 7.5 Other source research dependencies

Telegram’s current published API/content terms describe broad AI-related restrictions and context-specific consent exceptions. Applicable use must be established for that source and purpose; a successful connection alone does not establish eligibility for model processing. Keep an independent path using owned or appropriately permissioned material.[^6]

NASA FIRMS exposes product/date availability and bounded per-request windows; USGS’s service limits differ from the previous app’s display caps. Discover and display these adapters’ capabilities instead of encoding old interface limits as product truths.[^7][^8]

Copernicus’s catalogue supports spatial/temporal acquisition discovery and collection metadata; OASIS CAP distinguishes advisory updates/cancellations, validity and urgency/severity/certainty. These are useful source semantics without prescribing a backend.[^9][^10]

## 8. Map, time, entities, and contextual layers

### 8.1 The working surface

**MAP-01 — Should, first release.** Use a persistent scope bar, collapsible layers/filters, a map or linked results list, an optional timeline, and one resizable evidence inspector. Claims, incident context, entity history and assistant evidence use that inspector rather than accumulating overlapping drawers. A hover preview is transient: show a readable source name and a short headline, with no internal IDs or revision hashes. Location-choice rows follow the same rule. Substantive reading has a stable selected record and a linkable location. The normal feed offers All messages (every retained message within the shared filters) and Event reports (processed messages containing a reported occurrence, including those without a resolved location). Event reports are source claims, not verified or deduplicated incidents; unprocessed posts and planned/denied/historical-only claims do not enter that filter.

**MAP-02 — Must, first release.** Provide zoom, fit results, return to previous view, north/reset bearing, scale, coordinate search/copy, and named views. First use offers a neutral overview or a chosen area of interest. Later sessions restore the last useful view. Data arrival does not move the camera, reorder the item being read, or replace its content without an update indicator.

**MAP-03 — Must, first release.** The main collection selector identifies a published analytical collection, saved view, or investigation. Advanced mapping allows choosing coordinate, time, category, identity and label fields from a dataset, with a preview and validation. Unknown fields, incompatible units or invalid coordinates produce actionable explanations. Changing compatible datasets preserves the selected interval; incompatible scope prompts an explicit adjustment rather than quietly resetting time.

**MAP-04 — Must, first release.** Layer status separates newest observation, last acquisition, last successful refresh, current request state, coverage and result completeness. State the drawing subset and selection rule if only some matches are rendered. Counts and summaries identify whether they cover all qualifying records, a sample, or loaded records. Dense rendering must not turn an unreported sample into an analytical population.

### 8.2 Symbols, lists, and uncertainty

**MAP-05 — Must, first release.** Use stable semantic category symbols across sources, datasets, sessions and time. The initial vocabulary is configurable and versioned, with explicit unclassified and other states. Labels and shapes supplement color. Category frequency must not reassign meaning or urgency. A legend explains symbols, selected filters, cluster units and any confidence/assessment rendering mode.[^11]

**MAP-06 — Must, first release.** Clusters describe what they count: reports, incident groups or observed entities. Expose mixed-category composition and expand coincident points through a list or fan. A cluster is a rendering aid, not proof that items belong to one incident. Selecting a rare item from the list locates it even when aggregate rendering would conceal it.

**MAP-07 — Must, first release.** Show reported geometry or an appropriately imprecise estimate. Distinguish exact point, named facility, locality, administrative area, broad bounds and unresolved location. Display uncertainty when selected and when significant at the current scale, not only above a universal distance threshold. A centroid is labeled as a representative point, never disguised as an exact occurrence.

**MAP-08 — Must, first release.** The linked list includes geographically unresolved evidence when the scope permits it. Use explicit location-status filters and a clear unplaced section/count. Default chronological sorting can switch to updated or needs review. Offer pagination or progressive loading with counts; an arbitrary latest-N list must not define the universe used by the assistant or brief.

**MAP-09 — Must, first release.** Active filters remain visible in every display mode. Facet counts consistently state whether they are calculated before or after that facet's filter. A source exclusion applies to country summaries, incident evidence, attention lists and assistant context under the same scope. Incoming results appear behind a New items control while the analyst is reading.

### 8.3 Time modes

**MAP-10 — Must, first release.** Live uses a named trailing interval; first-release Replay fixes an event-time interval within a selected retained snapshot or current complete publication, as defined in TIME-02. The timeline shows the selected time basis and timezone, includes date/time inputs and step controls, and displays coverage gaps. Time brushing is an additional input, not the only one. Show an event's uncertain interval rather than assigning false precision for plotting convenience.

**MAP-11 — Must, first release.** Replay hides current-only layers by default. The analyst may explicitly add them as Current context; visually distinguish them and exclude them from historical conclusions. A historical position with today's vessel metadata labels the mismatch. The product cannot claim to show a complete past situation merely because its report dots are historical.

**MAP-12 — Must, investigation release.** Compare displays two explicitly named scopes, usually identical geography/filters with different intervals. It identifies changed sources, availability and interpretation versions. The analyst can lock like-for-like source coverage. A selected vessel's local track cursor never silently replaces the workspace's global interval. Known-at-the-time replay follows TIME-03 and includes later interpretation only in a separately identified retrospective mode.

### 8.4 Reports, claims, and context

**MAP-13 — Must, first release.** Report selection opens original/translated text, media, times, source, derived claim fields, geographic reasoning, revision state and analyst notes in the shared inspector. Each claim's assessment is separate from its location correction and review state. Keep context actions to pin evidence, inspect exact citation, correct location, compare reports, and begin an investigation.

**MAP-14 — Must, first release.** Related context has three named relationships: explicitly linked evidence, possibly related evidence, and nearby observations. Show the basis, radius/time window if used, and unresolved dependency. Explicit incident members open by identity; spatial search discovers candidates. Reused media, propagation, contradictions and claim changes link to exact records. Search outside the original scope labels its expansion.

### 8.5 Observation layers

**LAYER-01 — Must whenever a layer is introduced.** Every layer supplies a consistent legend/status card: provider, evidence family, units, observation and acquisition times, temporal support, coverage, aggregation, exclusions, source attribution and detail action. Unconfigured or unauthorized options explain their state. Turning a layer off changes presentation unless the analyst also changes evidence scope; this distinction is visible.

**LAYER-02 — Must when thermal observations are introduced.** Show sensor/product, acquisition time, detection quality in the provider's terms, available measured power, observed footprint/resolution, revision and source reference. A thermal observation does not establish cause, attack, casualties or a precise damage boundary. Default recurring industrial detections to dimmed with counts and an explicit hide option. Repetition is useful context, not a sufficient reason to erase an observation. NASA's documentation describes product-specific confidence, false detections and missed detections; do not convert those into a universal truth score.[^12]

**LAYER-03 — Must when earthquake/advisory layers are introduced.** Earthquakes retain magnitude type, depth, occurrence/update time and review status. Institutional advisories retain issuer wording, affected geometry, validity and update/cancel links. The issuer's severity is not the same scale as the analyst's watch urgency. Expired warnings remain accessible as history where retained but do not look currently active.

**LAYER-04 — Must when maritime observations are introduced.** Use KeelWatch for vessel observations, details, nearby evidence and collected history. Select a vessel from its latest qualifying position and open immediately available details while enrichment/history load separately. Show provider-qualified identifiers, identity certainty, reported class/flag/destination and available observation metadata. Distinguish vessel counts from aggregate density cells. Categories are searchable groups with a compact active-filter summary. Missing identity and satellite reception are independent properties.

**LAYER-05 — Must when movement history is introduced.** Tracks show actual timed fixes, history coverage, reporting gaps and provider. Missing intervals remain gaps or clearly styled inferred connections. A cursor identifies the nearest actual observation. Show reported versus derived destination/route separately, preserve historical metadata when available, and allow pinning exact observations into an investigation. Unavailable history does not prevent inspecting the current point.

**LAYER-06 — Must when aircraft observations are introduced.** Aircraft cards distinguish observed position/altitude/speed, supplied flight/airframe identity, and inferred route/operator/mission classifications. State units and observation time. Operator summaries describe observed contacts in available coverage; they are not fleet totals or unqualified surge indicators. Use the existing custom Flightradar24 connector and its actual observed capabilities. Keep the live viewport, short position buffer and selected-flight trail distinct. Related images and enrichments use Relic’s proxy-controlled path.

**LAYER-07 — Should, domain expansion.** Offer neutral movement-review candidates such as long observation gaps, loitering-like paths or identity changes, with thresholds, evidence and alternative explanations. Analyst-added distance/bearing/time measurements and annotations are more immediately useful than an unexplained suspiciousness score. Defer autonomous intent, mission, attack and evasion judgments. Popular-flight discovery and decorative aircraft photos are optional enrichments outside core attention ranking.

### 8.6 Imagery and references

**IMG-01 — Must, investigation release.** Search imagery by area, acquisition interval, collection/product and available quality metadata. Show acquisition identity, footprint, time, resolution, rendering, valid-data mask and source. Distinguish no acquisition, cloudy/no-data acquisition, unavailable asset, failed service and unsupported history. A basemap is context unless its evidence vintage is known and selected deliberately.

**IMG-02 — Must, investigation release.** Compare two pinned acquisitions using a synchronized pair or swipe, with non-drag controls. Keep dates, coverage, credit and rendering visible. Identify mismatched bands, processing, alignment, resolution or illumination that could explain apparent change. An analyst can annotate a region as a visible-change candidate and cite both acquisitions; automatic damage classification is deferred.

**IMG-03 — Must, investigation release.** Do not present whole-scene/tile cloud percentage as local visibility. Where available, show cloud/no-data information for the area being inspected. A mosaic identifies the acquisition times contributing to it. Copernicus explicitly describes tile-level cloud filtering and exposes masks and observation metadata; the brief requires honest inspection, not a particular imagery service.[^13]

**REF-01 — Must, first release.** Reference cards show class, names/aliases, geometry, source/edition, update/effective time where known and precision. Support points, lines and polygons. Initial names and administrative boundaries can expand to airports, ports, facilities, energy assets, cables, corridors, chokepoints and analyst-defined areas when justified. Analyst annotations remain distinguishable from externally sourced reference facts.

**REF-02 — Must, first release.** Nearby reports do not automatically mean a facility was struck or an asset was involved. Any attack/affected assertion links to a dated incident and assessment, retaining uncertainty. A reference shown in replay states whether it was valid then or is latest-known context. Selection emphasis, historical claim styling and current warning styling remain distinct.

The recommended watch inbox borrows established grouping, deduplication and timed-silence behavior from alert-management systems. Applying it to analytical work is a design analogy: not every useful finding requires immediate interruption. Default to inbox/digest, with interruption reserved for analyst-selected conditions.[^14][^15]
## 9. Workbench, processing, and operational understanding

### 9.1 Recipes and the expert graph

**PIPE-01 — Must, first release.** A processing recipe describes named inputs, ordered/connected transformations, output definitions, versions, schedules and work limits. Provide a few editable starting recipes: reporting to inspectable claims and locations; structured observations to a map; and institutional advisories to watches. Structured inputs should bypass unnecessary language-model interpretation.

**PIPE-02 — Should, first release.** Use a readable step list with input/output examples and expandable settings for routine work. A visual data-flow/lineage view is required in the first release under OPS-02; advanced graph editing can follow. An expert editor reveals branching, joins and multiple outputs. All views describe the same saved recipe and its recorded executions. Preserve direct controls for expert users; do not require a graph drag operation or chat conversation for a basic field mapping.

**PIPE-03 — Must, first release.** Each step shows input/output counts, exclusions, failures, unresolved items and representative changed examples. A preview names its sample and selection rule, with a way to include edge cases. Explain row multiplication and missing values. Excluding material from an event output must not delete the source evidence from the Library.

**PIPE-04 — Must, first release.** Create, duplicate, rename, archive, save draft, validate and inspect recipe history. Connections have named, compatible input/output shapes. Reject cycles where the recipe model does not support them. Removing or changing a step identifies dependent outputs and drafts. A layout-only change cannot alter analytical meaning.

### 9.2 Complete transformation capability catalogue

The following preserves all 25 capabilities of the original concept. These are product operations, not required class names or implementation modules. The phase label **First, if selected** means eligible for the initial recipe set, not that every listed operation must ship immediately. Only operations needed by the selected initial reporting/import, structured-observation and advisory pathways are first-release obligations. Full general composition can follow. All 25 operations remain in the target capability catalogue, and every introduced operation obeys lineage, preview, failure and identity contracts.

| # | Capability | Required observable behavior | Phase |
| --- | --- | --- | --- |
| 1 | Dataset input | Select a collection/version or deliberate latest binding; show its schema and availability | First, if selected |
| 2 | Select fields | Keep chosen fields without breaking retained evidence references; preview output shape | First, if selected |
| 3 | Filter rows | Apply explicit conditions; define missing-value handling and report excluded counts | First, if selected |
| 4 | Union | Combine compatible inputs; preview schema reconciliation and preserve source identities | First, if selected |
| 5 | Computed field | Derive a value from named inputs; explain expression errors and units | First, if selected |
| 6 | Expand a list | Produce child records with stable parent/element references; distinguish empty, absent and failed extraction | First, if selected |
| 7 | Exact deduplication | Deduplicate chosen identities/keys with explicit keep/merge policy and inspectable alternatives | First, if selected |
| 8 | Normalize labels | Map variant labels to a versioned vocabulary; preserve originals and unknowns | First, if selected |
| 9 | Assemble bursts | Combine related adjacent source fragments using visible time/source rules; preserve every constituent | Investigation |
| 10 | Fold quotes and replies | Keep quoted/original author and conversational relationships; distinguish new claims from repeated text | Investigation |
| 11 | Form content blocks | Assemble text/caption/media/extraction segments while retaining their order and exact source locations | First, if selected, for supported formats |
| 12 | Source profile | Summarize available languages, subjects, geography and coverage over a named interval; do not infer credibility or use a hidden location prior | Investigation |
| 13 | Aggregate | Group and calculate named measures, distinguishing rows, reports, origins and incidents | First, if selected |
| 14 | Join | Declare keys and expected cardinality; preview unmatched rows, duplicate keys and multiplicative effects | First, if selected |
| 15 | Sort and limit | State ordering, tie-breaking and truncation; a limit is not an unqualified complete result | First, if selected |
| 16 | Rename fields | Preserve meaning and provenance; flag collisions and dependent expressions | First, if selected |
| 17 | Cast types | Convert with visible failures, missing-value policy, units/timezone/locale handling | First, if selected |
| 18 | Geographic filter | Use declared point/extent/intersection rules and an explicit policy for unresolved geometry | First, if selected |
| 19 | AI extraction | Produce supported, attributed fields with per-field evidence and abstentions; retain non-event material | First, if selected, optional AI |
| 20 | Extract geographic mentions | Identify names and semantic roles such as origin, destination, occurrence, comparison and issuer | First, if selected, optional AI |
| 21 | Translation | Preserve original text, language and names/numbers/negation/modality; identify generated translation and uncertain passages | First, if selected, optional AI |
| 22 | Semantic repetition detection | Suggest shared content/origins with rationale and configurable policy; do not destroy originals or establish independent confirmation | Investigation |
| 23 | Geographic resolution | Offer supported location candidates, role, precision and abstention; use scoped references and corrections | First, if selected |
| 24 | Incident grouping | Propose associations with reasons, time/space/category assumptions and versioned membership; allow split/merge review | First, if selected, initially simple |
| 25 | Spatial association with references | Record geometry relation, distance/units and reference version; proximity alone does not establish involvement | First, if selected |

A reporting recipe can preserve/acquire → assemble supported content → extract/translate if eligible → identify location roles → resolve conservatively → suggest incident associations → publish. The analyst can inspect each transition. A structured observation recipe can validate units/time/identity → normalize → publish without AI. A quoted denial can remain searchable evidence even when it contributes no positive occurrence marker.

### 9.3 Draft, evaluate, publish, and recover

**RUN-01 — Must, first release.** Distinguish Validate/Dry run, Evaluate sample, Evaluate full candidate and Publish. Dry run checks configuration and retained data without new paid/model work or publication. Evaluation may perform bounded paid work, clearly labeled before starting. Display expected work, reusable results, limits and uncertainty in the estimate.

**RUN-02 — Must, foundation.** A run records exact input versions, recipe revision, reference/correction versions, enrichment policy revisions, model/provider identity when available, actor, start/end, requested and actual work, failures, outputs and publication decision. A running recipe is frozen to its selected revision; editing its draft does not mutate the in-progress run.

**RUN-03 — Must, first release.** Compare a candidate with the current publication before deliberate promotion: added/removed evidence, changed fields, locations, classifications, incident membership and unresolved/error counts. The depth of comparison can expand with later features, but a geographic shift must not be reduced to an unexplained row count.

**RUN-04 — Must, foundation.** Publish a coherent complete output version only when required work satisfies the declared completeness policy. Keep the previous complete result active after failed required work. Optional enrichment can be absent only when the output identifies it. Publishing an already approved routine recipe on new input may be automatic within its scope and budget; no per-report approval is required.

**RUN-05 — Must, first release.** Stop remains reachable during execution. It stops scheduling new work, attempts supported cancellation, states which in-flight work cannot be withdrawn, preserves reusable completed work and avoids incomplete publication. Resume names the retained input/recipe revision and remaining work. Provider charges already incurred remain visible.

**RUN-06 — Must, first release.** An unchanged complete dependency set—including evidence, recipe, reference, correction and enrichment-policy revisions—verifies freshness without creating redundant analytical versions or repeat alerts. Changed processing dependencies may require a new candidate even when source text is unchanged. Retry recoverable failures with bounded attempts/backoff. Deterministic failures do not loop indefinitely. Concurrent triggers coalesce or queue visibly rather than silently execute the same paid work twice.

**RUN-07 — Must, investigation release.** Distinguish Open retained result, Replay retained computation and Re-evaluate. Opening reads history. Replay uses retained inputs and outputs where possible, declaring missing dependencies. Re-evaluation produces a new candidate and may differ even with the same source text. Do not promise deterministic regeneration from a changing model or service.

**RUN-08 — Must, first release.** Restoring an earlier publication records a new publication decision while preserving later history. Preview affected watches and apply their explicit rebaseline/re-evaluation policy. A restored old result must not flood the inbox with historical matches presented as new occurrences.

### 9.4 Operational surfaces

**OPS-01 — Must, first release.** A concise health summary distinguishes acquisition freshness, backlog, processing failures, output publication, provider availability, reference coverage and watch delivery. Clicking a problem opens the relevant evidence or recovery action. Healthy operations stay quiet; analytical findings and maintenance problems occupy separate inbox categories.

**OPS-02 — Must, first release.** Provide an interactive visual data-lineage flow linking source instances, exact evidence/dataset revisions, recipe steps and runs, published outputs and downstream uses such as map layers, watches and cited briefs. Make it a primary Workbench view with Show lineage entry points from evidence, datasets, runs and briefs. Selecting a node or edge opens actual input/output records, transformation settings and run history. Provide a searchable keyboard-accessible list equivalent. Record-level provenance is required, not only a workspace-wide dependency diagram; follow the interaction contract below.

**OPS-03 — Must, first release.** Activity history records acquisitions, unchanged verification, publications, failures, retries, rule actions, corrections and exports with times, actor, result and related objects. Filter it and reopen relevant detail. Preserve actionable failure history beyond transient messages. Administrative actions should not crowd the main intelligence digest.

**OPS-04 — Must, first release.** Cost/usage includes estimates versus incurred amounts, unknown prices, active work and configured limits by source/task where available. Set bounded acquisition, enrichment, retry and concurrency budgets. A claimed hard cap cannot rely on an unbounded or unpriced request. If only a spend estimate is enforceable, label it as such and provide enforceable request/work limits.

**OPS-05 — Should, first release.** Machine/service details expose only useful operating facts: availability, capacity, queue pressure and actionable exhaustion. Additional host metrics belong in diagnostics. The system-flow display is useful for maintenance but does not need to be the default analyst homepage.

### 9.5 Visual lineage interaction contract

The first release provides a left-to-right flow: **source → retained revision → processing steps → published dataset → map/watch/brief use**. Reference releases, analyst corrections and AI task policies appear as side inputs to the steps that used them. Labels distinguish data derivation, reference lookup, report-copy provenance and analyst citation/selection; an arrow does not mean causation or independent confirmation. A brief may cite retained evidence directly without passing through an AI step.

Offer three explicit modes: **Recipe design**, **Latest published**, and **Selected run**. The design describes intended connections. The latter two show recorded revisions and execution state, with date/run/publication controls. A failed candidate is visible beside the last active complete publication; it cannot repaint that publication as failed or pretend its partial output replaced it. Opening a saved brief's lineage uses the exact revisions it cited, even after source, recipe, reference or model changes. Missing/expired inputs remain labeled gaps, without inventing a dependency or reconstructing forbidden content.

Select a node to inspect its purpose, exact revision, input/output schema and examples, counts, exclusions, unresolved/error counts, latest-success time, duration and available AI cost/model/provider details. Counts distinguish records, logical evidence items, model calls and bytes. Select an edge to inspect the input binding or field mapping and trace records through it. Follow a selected output back to exact supporting source revisions; follow an input forward to affected publications, watches and briefs. For filter/expand/join/dedup operations, show why a record was excluded, multiplied or grouped. Where an exact record/field relationship was not retained, say lineage is unavailable at that granularity; do not replace it with an inferred edge.

Provide fit-to-flow, pan/zoom, search, jump-to-selection, collapse/expand groups, upstream/downstream focus and a small overview for large graphs. Show state with text/icons as well as restrained colour: queued, running, succeeded, reused, failed, cancelled, blocked and stale. Animate only actual active work; stop/reduced-motion modes remain usable. Preserve layout and selection across progress updates. Expand a bounded neighborhood with explicit hidden-node counts rather than load every record into one canvas. The equivalent list exposes the same actions and relationships.

Viewing, expanding or moving graph nodes performs no acquisition, model call or recomputation. Run, retry, edit and publish remain explicit existing actions with their own work/impact previews. A planned recipe graph, recorded execution lineage and record-level trace share stable IDs and one authoritative model; layout is presentation state. Export a labeled diagram plus an eligible structured lineage manifest for the selected revision, retaining omissions, attribution and source restrictions. The later graph editor edits the existing recipe; it must not introduce a second execution engine.

## 10. Geographic resolution and reference management

**GEO-01 — Must, first release.** Resolve an event location from the report's meaning, not simply the first place mentioned. Preserve alternative candidates and roles: an issuer in one city can describe an incident elsewhere; an origin and destination can both be relevant. Location identity and spatial precision are separate. An unresolved output is acceptable and remains investigable.

**GEO-02 — Must, first release.** A correction can target one evidence item, a specified source/context, a language/name combination, or a deliberately broader rule. Preview affected items, competing rules and countries before applying. Default to the narrowest justified scope. Record actor, rationale, supporting evidence, previous result and correction revision; undo changes future interpretation without erasing the earlier decision.

**GEO-03 — Must, first release.** Corrections, gazetteer matches and AI proposals are attributed separately. A model's endorsement or high self-reported confidence cannot by itself authorize a persistent global rule. Resolve with AI creates proposals unless a previously enabled, bounded correction policy covers the operation. Ambiguous proposals remain available for review, not silently counted as resolved.

**GEO-04 — Must, first release.** The quality queue groups unresolved names, broad placements, conflicting candidates and suspected wrong-country results. Show counts with their denominator, current scope and sampling limits. Offer original text, context, candidates, map, proposed scope and impact in one review flow. Queue navigation returns to the original investigation after a decision.

**GEO-05 — Must, first release.** Reference packs have identity, source/license metadata, edition, represented time, geographic and feature-class coverage, approximate size, availability and active revision. Search/preview/acquire/validate/activate/replace/remove are distinct operations. Before publication, retain the resolved source asset/request, release and record identities, checksum, rights basis, geometry/units and a quality report covering schema, completeness, missingness and reviewable changes. Raw releases, normalized records, display geometry, crosswalk decisions and analyst annotations remain distinct. Candidate name/proximity matches cannot silently merge entities. A failed or partial update keeps the prior usable edition active; removals need a complete compatible snapshot or explicit source deletion. Show dependent analyses and missing-reference states. Inherited inputs stay visibly unverified and disabled in the normal baseline until reconciled; their undated flags cannot create current incidents. Specialized facility coverage is optional and openly incomplete. `reference/DATA_PLAN.md` and its catalogue define acquisition priorities and limits.

**GEO-06 — Should, investigation release.** Measure geographic quality with reviewed examples by language, location type and difficulty, including wrong-place rate, false precision and appropriate abstention. Coverage alone is not accuracy. Keep numeric place-match scores inside their documented meaning and do not present them as the chance the report is true.

## 11. AI assistance, evaluation, and delegated actions

### 11.1 Useful, inspectable assistance

**AI-01 — Must when AI assistance is introduced.** The assistant can search eligible retained evidence, explain a selection, compare claims, find contradictions, draft a brief, propose a watch and propose scoped corrections. These operations also have direct product controls where central to the workflow. The analyst must be able to collect, inspect, annotate and export without a functioning model service.

**AI-02 — Must when AI assistance is introduced.** Answers distinguish source reporting, derived interpretation, analyst judgment and unresolved questions. Support each material factual claim with exact evidence; label inference and cite its premises. Preserve meaningful disagreement. Offer Inspect evidence, Check contradictions and Show what was not checked. The last action names retrieval scope, exclusions, caps, failed tools and stopping reason.

**AI-03 — Must when AI assistance is introduced.** Conversation state is scoped and recoverable, but chat is not the sole store of analytical memory. Saved investigations, decisions, notes and brief revisions are durable explicit objects. Cross-investigation memory is opt-in and inspectable. Deleting a conversation does not silently delete a saved assessment or export.

**AI-04 — Must, foundation.** Evidence text, attachments, OCR, retrieved pages and tool results are untrusted input. They cannot grant permissions, change source eligibility, expose credentials or replace rules by issuing instructions. Validate action capabilities and arguments independently of the model. Show an action's resulting state rather than treating the model's statement of success as proof. NIST and OWASP identify relevant confabulation and excessive-agency concerns; these requirements address observable failure modes without claiming they eliminate all risk.[^16][^17]

### 11.2 Autonomy that respects the analyst's attention

**AI-05 — Must when AI assistance is introduced.** Use four action classes: Inspect performs bounded authorized reads; Navigate makes reversible requested view changes; Draft prepares a watch, correction, recipe or brief; Commit changes durable operational behavior or releases material externally. Commit requires an explicit user action or a preconfigured policy that covers that operation, scope and limits. A model cannot broaden its own policy. Avoid repeated approval for already authorized routine work.

**AI-06 — Must when AI assistance is introduced.** Every delegated run exposes its task, permitted data/providers/actions, work/cost bounds, deadline and stop control. Record attempted and completed actions, changed objects and failed steps. On partial failure, name what changed and what did not. External messages, public sharing and destructive purges are never implied merely by a request to analyze evidence.

**AI-07 — Should when AI assistance is introduced.** Prefer predictable processing recipes for repeated extraction/translation and bounded adaptive investigation for open questions. Add further agent roles only when a tested workflow benefits from them. A universal autonomous-agent canvas and self-editing production recipes are deferred. The workflow-versus-agent distinction is supported by practitioner guidance; the specific product boundary is a recommendation.[^18]

### 11.3 Quality before throughput

The owner-selected text-model baseline is **GPT-6 Luna** (`openai/gpt-6-luna`) through OpenRouter; embeddings start with `openai/text-embedding-3-small`. These retain the existing useful choices. `CONFIGURATION.md` records routing and task-policy details. Initial configuration is not an evaluated Relic capability: the gates below still apply, and model changes produce new policy revisions.

**EVAL-01 — Must for every shipped AI capability.** Define success measures, serious failure categories, abstention behavior and representative reviewed examples. Translation checks names, numbers, negation and modality; extraction checks field-level support; geocoding checks role and false precision; grouping checks false merges; summaries check support, omissions and contradiction handling; actions check resulting state.

**EVAL-02 — Must when AI assistance is introduced.** Compare a candidate policy against the published baseline using a versioned example set and a held-out portion. Show sample size, relevant language/source slices, known failures, quality measures, cost and latency. Unmeasured is distinct from passed. A small initial set can be useful but cannot establish universal reliability; grow it from actual failure patterns.

**EVAL-03 — Must when AI assistance is introduced.** Use deterministic checks, human-reviewed examples and optionally model-assisted grading. Model self-endorsement is not independent verification. State task-specific promotion thresholds before comparing candidates. Serious failures block automatic promotion; an analyst may explicitly accept a documented tradeoff without erasing its failed result. Retain evaluation history with the promoted policy.

**EVAL-04 — Must when AI assistance is introduced.** Choose among quality-eligible policies by cost, latency, provider availability and permitted data use. A tokens-per-second or speed-per-dollar benchmark is diagnostics, not an analytical quality test. Show numeric confidence only with a named meaning and documented calibration/evaluation basis. Otherwise explain support, ambiguity, missing evidence or failure.

**EVAL-05 — Should, investigation release.** Suggest useful analyst corrections as future evaluation cases, retaining their rationale and review status. Monitor repeated errors and overrides by task/source/language. A correction is not automatically ground truth. Evaluate repeated trials where nondeterminism materially affects the task. Outcome-based evaluation is a useful research precedent; it does not dictate an evaluation vendor or model.[^19]
## 12. Workspace behavior and growth boundaries

### 12.1 Shared components and access

**UX-01 — Must, first release.** A common shell supplies workspace navigation, global search, current task/scope context, service connection state, theme/density preferences and workspace settings. Use consistent tables, detail panels, status labels, forms and feedback. Bright and dark themes may be offered; neither should change semantic colors or reduce contrast. An analyst can work comfortably without animated markers or decorative motion.

**UX-02 — Must, first release.** Every persistent form distinguishes saved, unsaved, saving, failed and conflict states. Preserve drafts across accidental navigation and recoverable interruptions. Explicit links take precedence over old saved view preferences. Delayed restoration cannot overwrite an intervening user action. Long tables have sorting, bounded loading, empty/error states and keyboard-accessible row actions.

**UX-03 — Must, first release.** Protect workspace access in its chosen deployment setting. Explain expired sessions and allow reauthentication without losing unsaved work. Hide credentials from logs, URLs, evidence exports and model input. Do not require multi-organization roles, invitation flows or enterprise identity administration for a single analyst.

**UX-04 — Must, first release.** Meet WCAG 2.2 AA for the applicable interface and provide equivalent paths for dense map/graph actions: keyboard use, visible focus, text labels, meaningful reading order, non-color cues, contrast and single-pointer alternatives to dragging. Date fields/step buttons supplement brushing; a list supplements crowded geographic targets; a step list supplements graph manipulation. Apply the standard's target-size exceptions accurately rather than forcing every map point to the same size.[^20]

**UX-05 — Must, first release.** Preserve focus during live updates, allow pause, and announce summarized changes rather than every marker movement. Support reduced motion. Narrow screens prioritize list/detail, then deliberate map/graph access; avoid squeezing every desktop panel together. A resized viewport cannot hide Stop, Save or a critical error.

**UX-06 — Must, first release.** Onboarding offers a small fictional, clearly labeled demonstration investigation and a bounded import/source setup. Show the complete path from evidence to assessment to saved brief without requiring paid accounts. Demo evidence is excluded from real analysis unless explicitly included. If the workspace has no data, provide a relevant next action instead of unexplained zero gauges.

### 12.2 Logical objects and durable boundaries

This is the minimum conceptual model, not a database design. Implementations may combine or separate storage while preserving observable distinctions.

| Object | Meaning and required relationships |
| --- | --- |
| Workspace | The analyst's configuration, policies, saved work and ownership boundary; future collaboration can extend this boundary |
| Source type/capability | Evidence meaning plus supported acquisition/history/retention/processing/export operations |
| Source instance | One configured upstream identity, feed, region or import origin, with scope and collection history |
| Evidence item/revision | What was acquired at a known time, its source identity, original content/measurement, availability and revision history |
| Report/observation | An authored account or measured observation derived from/accompanied by evidence; logical identity survives row expansion |
| Claim | A scoped assertion/denial/correction with subject, predicate, qualifiers, uncertain time/place and exact evidence references |
| Entity hypothesis | A person, organization, place or asset identity with names, identifiers, matching evidence and unresolved alternatives |
| Incident | A stable analytical grouping with versioned membership, time/place scope and merge/split relationships |
| Assessment | An attributed judgment about a specific claim/entity/incident revision, its supporting/conflicting evidence and rationale |
| Investigation | A saved question, scope, selected evidence, assessments, notes, alternatives and unresolved work |
| Brief/revision | A preserved explanatory artifact tied to investigation, evidence and interpretation cutoffs |
| Watch/rule revision | A saved question and condition evaluated under explicit scope, budget, baseline and attention policy |
| Alert episode | A grouped durable match history with exact evidence, material changes, review state and delivery attempts |
| Collection/dataset version | A named analytical or acquired result with schema, logical count, provenance and completeness |
| Recipe/run/publication | What was defined, what was executed on which retained inputs, and what result became current |
| Reference/correction revision | A dated external context or scoped interpretation rule with effects traceable into later results |
| Saved view/snapshot | Presentation plus analysis scope, explicitly either following latest data or pinned to retained versions |

**MODEL-01 — Must, foundation.** Preserve durable identities and revision links at these boundaries, including the accountable actor even when initially only one analyst exists. Distinguish saved user work from rebuildable derived outputs. Reprocessing can update derived candidates; it cannot silently overwrite an analyst's authored note or assessment.

**MODEL-02 — Must, foundation.** Store permission/retention restrictions with enough scope to govern dependent content. Removal must account for cached originals, extracted text, search entries, model inputs/outputs where restricted, exports controlled by the workspace and backups under its control. A policy-expired item cannot be revived by restoring an old backup. Disclose that externally downloaded copies may be outside the workspace's control.

**MODEL-03 — Must, first release.** Repeated actions and interrupted recovery cannot create duplicate logical reports, repeated watch activation, or silently replace an unrelated rule with a reused identifier. An update identifies its target and revision; a new draft is a new object unless explicitly linked to a replacement. Conflicting edits have a visible resolution even if they arise from one analyst using multiple sessions.

Source-dependence research supports treating copying relationships as analytically material; it does not supply a ready-made truth calculator. A practical first release should show known origin groups and unresolved dependence, then let the analyst inspect the evidence. Likewise, professional investigation guidance supports careful preservation and documentation without requiring every personal investigation to adopt a formal legal workflow.[^21][^22]

### 12.3 Quality and performance targets

**PERF-01 — Must, first release.** Define and publish a representative performance fixture before calling the product responsive. Include retained originals/derived records, several languages, large result sets, dense/coincident geography, unlocated reports and degraded providers. Measure on the intended analyst environment, with cold/warm state and concurrent acquisition identified. A benchmark of an empty map is insufficient.

**PERF-02 — Should, first release.** Start with a provisional fixture of 50,000 report revisions, 20 configured source instances and a 10,000-match investigation, including dense points and unresolved locations. Proposed targets are local feedback within 100 ms, retained evidence opening within one second at the 95th percentile, and an ordinary scope update within two seconds at the 95th percentile. These are design budgets to validate and revise openly, not observed performance or universal requirements for every provider request. Long work shows progress and remains cancellable.

**PERF-03 — Must, first release.** Query limits, drawing limits, provider quotas and work caps are explicit and independent. Sampling or pagination preserves truthful totals and supports reaching any qualifying evidence. Monitor memory/storage exhaustion, failed saves and recovery, not only request latency. An external provider delay cannot freeze the entire workspace.

**PERF-04 — Must, first release.** Reserve execution capacity for interactive reads, save and stop while acquisition and bulk work run. Bound queues by bytes and item count, define overload behavior and expose backlog. CPU work and synchronous I/O cannot monopolize asynchronous request handling. Cancellation has an acknowledged state and actual worker interruption/cooperation; abandoning a future alone is insufficient. Protect committed saves and publication atomicity.

**PERF-05 — Must, first release.** Apply live presentation updates incrementally by stable identity, retain selection/camera/focus and reject obsolete scope responses. Carry scope/publication/sequence identity through requests and notifications; a dropped sequence triggers a resnapshot. Bound per-client event queues and allow coalescing of presentation updates while preserving durable source revisions and alert obligations. Map, list and totals cannot silently show different publications as one completed answer.

**PERF-06 — Must, first release.** Demonstrate the responsiveness budgets under concurrent ingestion, one bulk job, a slow source, rapid scope changes and a worker failure. Record the environment and separate normal load from synthetic overload. Memory must remain within declared budgets, stop/save remain usable and acquisition gaps remain explicit. Repeat the relevant checks after architectural changes; a connector smoke check or Rust microbenchmark does not establish application performance. `ARCHITECTURE.md` supplies the initial workload and measurement contract.

**QUALITY-01 — Must, first release.** Use acceptance fixtures for source preservation, scope agreement, exact citations, correction/merge history, watch deduplication, cancellation and restore. These invariants require deterministic checks where feasible and real interface walkthroughs. AI-dependent content also requires reviewed examples; a successful request status is insufficient.

**QUALITY-02 — Must, first release.** Report pilot outcomes against the analyst's jobs: time to locate original support, time to produce a cited brief, unsupported material claims in reviewed briefs, geographic false precision, repeated/unhelpful interruptions, unnoticed coverage gaps and recovery success. Do not use number of layers, tokens generated or alerts fired as primary product-success metrics. Establish baselines during pilot use; no universal numerical benefit is claimed here.

**QUALITY-03 — Should, investigation release.** Add a portable evaluation bundle containing fictional or permissioned evidence, expected invariant outcomes and reviewed examples. Another implementation should be able to demonstrate equivalent behavior without copying this app's internal architecture. Examples must include failure and ambiguity, not only attractive successful screenshots.

## 13. Delivery sequence and explicit exclusions

### 13.1 A complete first release before broad expansion

Foundation is a set of early decisions and invariants, not a separate launch. Requirement phase labels mean the behavior is due when that capability ships. In particular, every LAYER requirement applies when its optional module is introduced; it does not require shipping every layer in the first release.

| Stage | Deliverable | Exit evidence |
| --- | --- | --- |
| Foundation | Shared scope; evidence/claim/assessment distinctions; durable identity/revisions; time/availability semantics; provider eligibility; coherent publication | A small fixture survives source edit, interrupted acquisition, relocation, merge/split and retention expiry without losing or falsifying its history |
| First release: complete analyst loop | The custom X reporting path and imports; structured observations; geographic references; one institutional advisory integration; Watch, Investigate, Library, Sources and a compact Workbench; optional evaluated AI; saved briefs/export/restore | Analyst imports or collects evidence, creates a watch, inspects a meaningful change, assesses it, exports a cited brief and restores the retained investigation |
| Investigation release | Known-at-the-time replay; richer comparison; imagery pair inspection; source-aware assembly and repetition analysis; better version/brief differences; more reference/quality tools | Analyst reconstructs an earlier assessment without later evidence leaking in and explains a changed conclusion with exact sources |
| Domain expansion | KeelWatch ship and custom Flightradar24 aircraft modules; more sensor/context source types; movement-review candidates; richer expert graph controls | Each module demonstrates useful incremental evidence for a named task, accurate coverage/time semantics and acceptable operating cost |
| Later experiments | Radar/other imagery, validated change detection, optional credentials, entity graph exploration, additional agent roles, collaboration | A specific user need and evaluated benefit justify the added complexity; no new opaque truth or threat score |

The selected integrations must retain their described acquisition paths whenever their corresponding source/layer is shipped; source choice is not an open evaluation task. The first release can implement transformation operations through a small set of configurable recipes before exposing a universal node canvas. Optional context integrations such as earthquake and thermal feeds can be added once the core workflow passes; they should not delay the first complete evidence-to-brief path. Audio/video originals may be retained where supported while advanced transcription/alignment remains a later capability. Every introduced format must expose its supported reading/citation behavior.

### 13.2 Things to remove or refuse as defaults

Do not carry forward geographic/time/filter inconsistencies, nearby citation substitution, fixed top-five symbol assignment, automatic copying-as-corroboration, truth inferred from a location confidence badge, incident identities that break after backfill, or current sensor layers passed off as historical evidence.

Do not mandate a specific map starting location, English as the only future working language, original widget dimensions, arbitrary similarity/radius/window constants, fixed latest-row caps, or dependency versions inherited merely from the former implementation. Apply the owner’s Rust requirement and initial storage decision through the architecture contract. Supply sensible editable examples and explain their effects. The selected X and Flightradar24 protocol behavior and mandatory proxy egress are explicit compatibility requirements. Other previously observed implementation limits remain subject to the target contracts.

Do not make popular flights, full-world surveillance, speculative threat scoring, autonomous external publication, a general-purpose arbitrary-code workbench, a node marketplace, enterprise roles, or perfect multimodal authenticity detection first-release goals.

### 13.3 Defaults, uncertainties, and decisions to validate

| Decision | Recommended starting assumption | What could change it |
| --- | --- | --- |
| Primary workflow | Watch-led investigation for one advanced analyst | Task observation shows sustained map patrol is the dominant job |
| Source bundle | Existing custom X and Flightradar24 integrations, KeelWatch ships, imports and names/boundaries | Additional source types may extend the platform; these three provider choices are already decided |
| Domain | General evidence-led geopolitical/infrastructure monitoring; no promise of complete world coverage | A narrower maritime, aviation, disaster or investigative speciality is chosen |
| Geographic scope | Analyst-chosen areas; no compulsory default region | A recurring operational geography becomes clear |
| Display language | Analyst-selected; preserve original scripts and expose supported translation quality | Evaluation demonstrates adequate support for additional working languages |
| AI use | Optional, bounded and task-evaluated; start with the model and routing baseline in `CONFIGURATION.md` | A later evaluated policy addresses privacy/eligibility, cost, quality or unavailable capabilities; never silently substitute a model/provider |
| Notification | In-app inbox and digest first; external delivery optional | The analyst explicitly needs a particular timely channel and its failure behavior is tested |
| Retention | Preserve saved work and eligible evidence; source-specific expiry takes precedence | Specific storage budgets or agreements require shorter/different preservation |
| Advanced layout | Recipes plus graph, one inspector, map/list parity | Task walkthroughs reveal a better arrangement without losing direct expert control |
| Collaboration | Record actor and workspace boundaries now; shared editing later | A second analyst needs concurrent work, review or controlled sharing |

X and Flightradar24 use the owner’s existing integrations; KeelWatch is the owner’s maritime source. Their behavior is grounded in the inspected Recall implementation and the explicit Relic requirements in §7.4. GDACS and maritime-security warning services remain candidate institutional adapters whose current endpoints, coverage and conditions need verification before commitment.

## 14. Capability coverage map

This table locates each capability family in the target design. It is an index for the new implementation, not a dependency on an older specification. Detailed behaviors and acceptance criteria live in the referenced sections.

| Capability family | Target behavior and location |
| --- | --- |
| Product purpose and analyst journeys | Keep evidence/geography/monitoring; add durable question → judgment → brief in §§1–5 |
| Information model, identity and time | Strengthen with claim/assessment separation, origin dependence, revision history and availability in §§3 and 12 |
| Collection, preservation, backfill and media | Keep bounded durable acquisition; add operation eligibility and rights-aware expiry in §§6–7 and 12 |
| Reporting transformation and all 25 nodes | Keep equivalent operations; replace fixed defaults and mandatory graph authoring with recipes in §9 |
| Complete publication and automatic execution | Keep; add sample/compare/publish/stop/resume and restored-version semantics in §9 |
| Resolution, correction proposals and incident corroboration | Keep conservative placement; replace model-approved global fixes and source-count truth shortcuts in §§3, 5 and 10–11 |
| Context, changes, attention, situation reports and Copilot | Unify scope; separate discovery from incident membership; persist briefs; make exact support central in §§4–5, 8 and 11 |
| Shared shell, access, theme, tables, errors and drafts | Keep consistent components; strengthen recovery, accessibility and direct controls in §§3 and 12 |
| Observatory lineage canvas, node/edge detail, dataset drawer and activity | Move into Workbench; preserve graph/list navigation, exact provenance and activity in §§6 and 9 |
| Engine flow/freshness/quality/machine bands | Keep actionable health and diagnostic depth; demote decorative aggregate metrics in §9 |
| Provider serving/routing, benchmark/recent calls, Pin winner | Keep availability/cost/latency diagnostics; replace quality-blind promotion with evaluated task policies in §11 |
| Geographic triage, evidence/candidate selection, scoped forms, AI proposals and history | Keep and connect to an investigation; add impact and narrow scope in §10 |
| Reference import/install/activation, name index and update state | Keep explicit revisions and dependencies; remove hidden refresh obligations in §10 |
| Source rows, status/reliability, backfill/pause/resume | Keep; describe actual returned/physical coverage and operation states in §7 |
| Telegram discovery, X reader/handle monitor/credential swap, maritime region setup | Preserve the custom X reader, validated token replacement and proxy-pinned sessions; use KeelWatch region setup and source capabilities in §7 |
| Dataset catalogue/provenance/version table/preview/query | Keep and make retained versions actually inspectable; add import/export/restore and differences in §6 |
| Pipeline identity, palette, canvas, ports, toolbar, inspector, pickers, expressions and structured editors | Keep equivalent expert authoring/validation through a shared recipe definition in §9; accessible forms/step paths in §12 |
| Pipeline output declarations, auto-run, validation errors, drafts, sample previews and run history | Keep; add coherent candidate/publication and budgets, in §9 |
| Map orientation, basemap, dataset/field selection, coordinate display and loading | Keep with named views, stable scope and truthful sampling in §8 |
| Category glyphs/chips, confidence mode, source/text/review filters, clusters and stack fan | Keep access patterns; replace frequency-dependent semantics and ambiguous confirmation in §§3 and 8 |
| Event feed, popup, media loader, annotations and Context Lens | Consolidate into linked list and shared inspector; exact members and separate nearby context in §§5 and 8 |
| Timeline, histogram brush/playback and historical context | Add explicit Live/Replay/Compare, non-drag controls and cutoff semantics in §§3 and 8 |
| Thermal, earthquake, disaster and maritime-warning status/cards | Keep distinct source meanings; recurring thermal detections dim rather than automatically disappear in §§7–8 |
| Imagery pass choice, fade/swipe/blink | Keep dated comparison; add quality/masks/alignment, non-drag controls and annotation in §8 |
| Vessel categories, coverage, quick/detail/evidence/analytics views and track strip | Keep modular detail/history; strengthen gaps, count/identity semantics and cautious inference in §8 |
| Aircraft categories, details, trails, operator summaries and popular flights | Keep evidence and coverage; neutral operator labels; remove popularity from core attention in §8 |
| Reference classes/cards, facility status, lines/polygons and nearby glows | Keep context; replace unqualified attack styling with sourced dated claims in §8 |
| Accelerating areas and country-only activity | Move to comparable reporting-change analysis; preserve unresolved geography and qualify source changes in §§4 and 8 |
| Copilot trace/actions/watch drafts, map restoration/deep links and external-agent view control | Keep acknowledged scoped actions and exact links; explicit identities/policies prevent silent replacement in §§3 and 11–12 |
| Alerts rules/predicates/dry run/drafts/fired table | Expand into watches and actionable grouped inbox; preserve durable evaluation/delivery separation in §4 |
| Source taxonomy: social reporting, registered regions, movement, hazards, imagery and references | Preserve evidence families while separating providers, transports and capabilities in §§7–8 |
| Persistence/fidelity boundaries and reconstruction tests | Replace observed gaps with target invariants, explicit deferrals and adversarial acceptance in §§12–15 |
| Component checklist and implementation-file provenance | Replace a technology-linked checklist with this coverage matrix, behavior IDs and source citations; the standalone Relic clients, tests and source-interface guide contain the selected connector behavior without requiring the original repository |
## 15. Acceptance scenarios

Use fictional or permissioned fixtures that include original text, revisions, expected identities, time intervals, geometry, source relationships, coverage failures and some ambiguous judgments. These scenarios define observable behavior; they do not prescribe testing tools. Run a scenario when its corresponding capability is introduced. A deferred module does not block first-release acceptance.

| ID | Scenario and expected result | Main requirements |
| --- | --- | --- |
| A01 | Import permitted text and a table with invalid coordinates, missing times and mixed units. Valid evidence remains available; rejects/uncertainties are explained; originals and mapping choices are retained | SRC-01–08, EVID-01–02, PIPE-03 |
| A02 | Interrupt acquisition after saving evidence but before confirming its checkpoint. Resume without losing or duplicating logical reports. A later revision has the same source identity and new revision | SRC-07, ID-01, MODEL-03 |
| A03 | Pause a source, experience a rate limit on another, and observe a healthy empty response on a third. Their states differ and existing retained evidence remains readable | SRC-03, SRC-06–09, STATE-01–02 |
| A04 | KeelWatch returns a partial, truncated or grid-aggregated regional capture. Preserve the last complete version and report the limitation. A complete empty capture clears current membership; full returned counts still do not prove global physical coverage | SRC-08–09, KW-01, LAYER-04 |
| A05 | A provider cannot replay a connection gap. Reconnection resumes current acquisition and records the missing interval; the UI does not announce full catch-up | SRC-07, MAP-04 |
| A06 | Permit collection but prohibit external-model transmission for one source. Viewing works; affected AI requests are blocked or omit it explicitly. A different eligible source still works | SRC-04, AI-04, STATE-01 |
| A07 | Expire evidence supporting a saved brief, then restore an old backup. Expired content is not reintroduced; permitted residual references identify the unavailable support and replay limitation | EVID-06, LIB-07–08, MODEL-02 |
| A08 | Exclude one source and set category/time/location filters. All surfaces agree; unplaced items are counted. Existing incident summaries exclude claims supported only by the excluded source; preserved wider-scope assessments retain their scope label | SCOPE-01–03, MAP-08–09 |
| A09 | Pan an area-based investigation, then explicitly enable Search this view. Only the latter changes geographic scope. A previously saved brief keeps its original scope | SCOPE-02, MAP-02 |
| A10 | Summarize the current scope, then ask a wider question. The first excludes outside evidence; the second names the changed scope and citations | SCOPE-04, AI-02 |
| A11 | Open a citation to an unlocated report outside the loaded map sample. The exact revision and supporting passage open; no nearby item substitutes | EVID-02, READ-01–03, MAP-14 |
| A12 | Twenty publishers repeat one upstream statement and three have unknown dependence. Show publication count, observed origin groups and unresolved dependence; no automatic twenty-source confirmation | EVID-03, SRC-05, PIPE catalogue 22 |
| A13 | A denial, historical image, future plan and failed extraction enter collection. All remain searchable; extraction does not turn them into positive current events or erase them | EVID-04, READ-03–04, PIPE-03 |
| A14 | A report says a speaker in one city described a launch from a second toward a third. Preserve roles and uncertainty; do not locate every claim at the speaker's city | GEO-01, PIPE catalogue 20/23 |
| A15 | Correct a name shared by two countries for one source/context. Preview affected items; only the chosen scope changes. Undo retains the original decision and later reversal | GEO-02–03, ID-03 |
| A16 | Backfill an earlier report, merge two incidents and later split them. Old citations still resolve; memberships are versioned; whole-incident assessments are not blindly copied to successors | ID-01–02, INV-04, MODEL-01 |
| A17 | Reacquire a changed source post and later observe its removal. Distinguish upstream removal from continued permitted local availability; a saved assessment remains tied to the version it used | EVID-06, TIME-01–02, BRIEF-02 |
| A18 | Replay an earlier interval with a current-only aircraft layer configured. Hide it by default; explicit current context is labeled and cannot support a historical location conclusion | MAP-10–11, LAYER-01 |
| A19 | In the investigation release, reconstruct what was known at an earlier cutoff after later reports and a location correction have arrived. Exclude both the new reports and the later interpretation from the historical view | TIME-03, MAP-12, RUN-07 |
| A20 | A rare category becomes common and another disappears. Existing symbol meanings remain stable. Coincident points and unclassified material remain selectable through a list | MAP-05–08 |
| A21 | A thermal detection appears near a refinery and recurs. Display detection metadata and recurrence; do not automatically label an attack or hide the change | LAYER-02, REF-02 |
| A22 | An advisory is updated and then cancelled; an earthquake magnitude is revised. Current applicability changes without double-counting the same episode or physical event | SRC-11, LAYER-03 |
| A23 | Compare imagery with low whole-tile cloud percentage but cloud over the selected facility. Show local visibility limits and exact scenes; do not infer damage from the invalid comparison | IMG-01–03 |
| A24 | Open a vessel with sparse history and an aircraft with failed detail enrichment. Quick evidence remains readable, gaps are explicit, and current metadata does not overwrite selected historical evidence | LAYER-04–06 |
| A25 | Add a prolific source and backfill old reports during an apparent activity rise. Qualify the comparison and expose stable-source counts; do not present collection change as an unqualified event surge | CHANGE-03–04 |
| A26 | Test/activate a watch: testing sends nothing and advances no active baseline; later new matches create episodes. With insufficient history, activation declares frequency unknown and a fresh baseline | WATCH-02–03, ALERT-06 |
| A27 | Fifty copied reports match the same watch; reprocess them twice. Apply the configured grouping and material-change policy without repeat interruption merely for copies/reprocessing | ALERT-03–05, RUN-06 |
| A28 | Acknowledge an episode, then receive contrary evidence and a delivery failure. Keep review, claim assessment and delivery separate; the correction remains visible and delivery retry retains alert identity | ALERT-01–05, EVID-04 |
| A29 | Draft a watch whose proposed identifier matches an existing rule. The existing rule is not replaced without an explicit targeted update and impact preview | MODEL-03, AI-05 |
| A30 | In the visual lineage view, select a joined output and trace its exact source/reference revisions and field mapping. A join multiplies each report by three matches; expansion produces zero children for one parent. Show multiplication/exclusions, preserve originals and logical report counts, and expose the same path through the accessible list | OPS-02, PIPE-03, catalogue 6/13/14 |
| A31 | A candidate policy is faster but reverses a negation and misplaces an ambiguous locality. The comparison exposes serious failures and blocks automatic quality-based promotion | EVAL-01–04 |
| A32 | Budget exhausts after 300 of 500 required enrichments. Keep the previous publication active, preserve the 300 successes and resume the selected revision's remaining work | RUN-02–05, OPS-04 |
| A33 | Stop while paid calls are in flight, then edit the draft. No new work starts; unavoidable completions/charges are recorded; no partial publication occurs; the run retains its original definition | RUN-02, RUN-04–05 |
| A34 | Change a correction/reference/recipe while evidence stays unchanged: produce a new candidate. In lineage, inspect a failed candidate beside the last active publication; a saved brief still traces its original inputs. Restore an older publication: preserve both histories and the decision, applying watch baseline policy without a historical notification flood | OPS-02, RUN-02–04, RUN-08, LIB-02 |
| A35 | A source passage tells the assistant to reveal a secret and alter a rule. It can be quoted as evidence but does not change permissions, expose credentials or execute the requested modification | AI-04–06 |
| A36 | Generate a brief with contradictory and unlocated evidence, edit it, export it and reopen it after source updates. Exact support, author edits, scope/cutoff and gaps remain understandable | BRIEF-01–04, INV-01–05 |
| A37 | Recover an eligible saved investigation in a test workspace. Notes, scope, evidence references, decisions and recipe definitions remain consistent; excluded material is disclosed | LIB-07, MODEL-01–02 |
| A38 | Complete source setup, exact evidence inspection, watch review, time selection and imagery comparison with keyboard and click-only controls. Preserve focus during arrivals and reach every primary action at narrow widths | UX-01–06, MAP-10, IMG-02 |
| A39 | A session expires during a recipe edit while a provider fails. Reauthenticate without losing the draft; distinguish save state, connection state and provider failure; retain the last usable results | STATE-02–03, UX-02–03 |
| A40 | Run the declared dense-data fixture with concurrent collection. Measure responsiveness and cancellation, identify full versus sampled counts, and publish actual results against provisional budgets | PERF-01–03, QUALITY-01–02 |
| A41 | Configure X without a proxy, or make the selected proxy unavailable. Login, token validation, timelines, media and catalogue refresh perform no direct upstream request; existing evidence remains readable | NET-01–03, X-01 |
| A42 | Use an X session for login, handle resolution and multi-page catch-up. All use one proxy. Rotate only by constructing and authenticating a new client between sweeps; a failed new token does not displace the working token | X-01–02 |
| A43 | Test old pinned posts, unreadable cursor pages and an exhausted X page budget. Only exact ordinary-post watermark overlap or a completed timeline ends catch-up; failures keep the checkpoint unchanged after any safely persisted partial pages. Exhaustion pauses ordinary polling; resume the saved interval without losing earlier pages, and disclose a gap if deliberately rebaselined | X-03, SRC-07 |
| A44 | Open a Flightradar24 feed, trail, image and supported enrichment with the primary proxy unavailable. Try only configured proxy alternatives; missing/exhausted proxies fail closed. Inspect browser requests to ensure all displayed assets are served by Relic | NET-01–03, FLIGHT-01–03 |
| A45 | Saturate Flightradar24 tiles or truncate a protocol frame/trailer. Bound subdivision, deduplicate flight identities and expose incomplete coverage; malformed/failed responses are not complete empty feeds | FLIGHT-02, STATE-01 |
| A46 | View ships with the map open and closed. KeelWatch regional capture continues independently; a 304 reuses only the exact cached request; tracks come from KeelWatch history, not from sampled map viewing | KW-01–02 |
| A47 | Run acquisition and one heavy import while opening evidence, saving a note and stopping work. Interactive capacity remains available, queue bytes stay bounded, cancellation reflects actual work and committed saves survive | PERF-01–04, PERF-06, RUN-04–05 |
| A48 | Rapidly change scope while responses arrive out of order, then reconnect after the notification replay window expires. Reject old generations, resnapshot on a sequence gap and preserve map/list/count publication agreement and selection | PERF-05, SCOPE-03, STATE-02 |
| A49 | Exceed the synthetic live-update rate, then fail an adapter task and crash/restart the application. Coalesce only presentation updates, expose evidence gaps, recover from durable checkpoints and retain responsive stop/save controls with no direct-network fallback | PERF-04–06, NET-01–03, SRC-07 |
| A50 | Preview inherited references containing an undated attack flag, a malformed trade-location code and a shared facility name. Preserve originals and show unverified review candidates; create no incident or automatic identity merge. Stage a publisher release with its provenance, then simulate a partial refresh and a later complete correction. Keep the accepted version active on failure and preserve old citations/annotations after replacement | GEO-05, REF-01–02, EVID-03, RUN-02 |

In addition to these scenarios, check citation coverage and claim support in reviewed generated briefs. A fixture must fail if a material assertion cites a nonexistent or unrelated item, even when the prose sounds plausible. This invariant can have a zero-tolerance test result without claiming a zero real-world hallucination rate. Keep unsupported content out of the saved brief or mark it explicitly for analyst resolution.

## 16. Research basis and sources

The proposed design combines evidence from primary standards, original research, official product documentation and first-party design guidance. The selected X, Flightradar24 and KeelWatch integration descriptions come from the existing Recall implementation and the owner’s requirements, not from researched replacement services. Online dependencies were checked on **10 September 2026**. Publication dates below describe the cited edition where known; “living documentation” means no stable publication date was asserted. Provider terms and capabilities require rechecking before a build relies on them.

The five-destination layout, phased source portfolio, shared inspector, provisional performance budgets and other interface choices are recommendations, not outcomes proven by comparative trials of this exact product. Standards define concepts or criteria; vendor documentation establishes that vendor's behavior; studies have their stated scope. Agreement across sources strengthens a design rationale but does not validate the whole platform. The most important remaining validation is an analyst completing the end-to-end workflow with representative evidence.

[^1]: NASA, [Appendix C: How to Write a Good Requirement](https://www.nasa.gov/reference/appendix-c-how-to-write-a-good-requirement/), Systems Engineering Handbook guidance, page updated 26 July 2023. Supports clear, verifiable, implementation-independent requirements; its full aerospace process is not required here.

[^2]: Nielsen Norman Group, [Progressive Disclosure](https://www.nngroup.com/articles/progressive-disclosure/), 3 December 2006. Foundational first-party UX guidance; not a trial of the proposed navigation.

[^3]: Office of the Director of National Intelligence, [ICD 203: Analytic Standards](https://www.dni.gov/files/documents/ICD/ICD-203.pdf), base directive 2 January 2015, linked edition includes amendments. Supports distinctions among information, assumptions, judgments, uncertainty and alternatives. Domain-specific standards do not certify this app.

[^4]: C2PA, [Content Credentials Explainer, version 2.4](https://spec.c2pa.org/specifications/specifications/2.4/explainer/Explainer.html) and [Technical Specification 2.4](https://spec.c2pa.org/specifications/specifications/2.4/specs/C2PA_Specification.html), 2026 edition. Explains provenance and validation limits; no requirement to adopt this technology or infer truth from credentials.

[^5]: W3C, [PROV-DM: The PROV Data Model](https://www.w3.org/TR/prov-dm/), Recommendation, 30 April 2013. Conceptual vocabulary for entities, activities, agents, derivation and revision; not a storage architecture or credibility system.

[^6]: Telegram, [API Terms of Service](https://core.telegram.org/api/terms) and [Terms of Service for Content Licensing](https://telegram.org/tos/content-licensing), current published terms checked on the research date. Material dependency for AI-related use and contextual consent. Applicability must be established for the actual source, purpose and agreement.

[^7]: NASA FIRMS, [Area API](https://firms.modaps.eosdis.nasa.gov/api/area/), living documentation. Product/date/query availability, not a universal history limit or validated causal interpretation of thermal observations.

[^8]: USGS, [Earthquake Catalog Web Service](https://earthquake.usgs.gov/fdsnws/event/1/USGS), living documentation. Query, update and result-limit semantics; the app's previous display cap is not the provider's universal limit.

[^9]: Copernicus Data Space Ecosystem, [STAC API](https://documentation.dataspace.copernicus.eu/APIs/STAC.html), living documentation. Acquisition discovery and metadata; STAC is an interoperability example, not a mandatory implementation stack.

[^10]: OASIS, [Common Alerting Protocol, version 1.2](https://docs.oasis-open.org/emergency/cap/v1.2/CAP-v1.2-os.html), OASIS Standard, 1 July 2010. Structured advisory identifiers, updates/cancellations, validity and severity dimensions; provider adherence must be checked.

[^11]: Esri, [Unique Values Symbology](https://pro.arcgis.com/en/pro-app/3.6/help/mapping/layer-properties/unique-value.htm), ArcGIS Pro 3.6 documentation. Demonstrates configurable category styles/grouping; does not establish a mandatory event taxonomy.

[^12]: NASA Earthdata, [FIRMS Frequently Asked Questions](https://www.earthdata.nasa.gov/data/tools/firms/faq), living documentation. Sensor confidence, detection limitations and interpretation context; no attack-attribution claim follows from a thermal point.

[^13]: Copernicus Data Space Ecosystem, [Sentinel-2 L2A Data Documentation](https://documentation.dataspace.copernicus.eu/APIs/SentinelHub/Data/S2L2A.html), living documentation. Tile cloud filtering, masks and observation metadata; local comparison validity requires inspection.

[^14]: Google, [Monitoring Distributed Systems](https://sre.google/sre-book/monitoring-distributed-systems/), Site Reliability Engineering book, 2016. Alert-actionability guidance; transfer to intelligence attention is an explicit design analogy.

[^15]: Prometheus, [Alertmanager](https://prometheus.io/docs/alerting/latest/alertmanager/), living documentation. Grouping, deduplication and silence concepts; its software is not prescribed.

[^16]: NIST, [Artificial Intelligence Risk Management Framework: Generative Artificial Intelligence Profile](https://nvlpubs.nist.gov/nistpubs/ai/NIST.AI.600-1.pdf), NIST AI 600-1, July 2024. Addresses confabulation, evaluation and human factors; a risk framework is not proof of a guardrail's effectiveness.

[^17]: OWASP, [LLM06:2025 Excessive Agency](https://genai.owasp.org/llmrisk/llm062025-excessive-agency/), 2025 guidance. Supports limiting functions, permissions and autonomy and independently enforcing authorization.

[^18]: Anthropic, [Building Effective Agents](https://www.anthropic.com/engineering/building-effective-agents), December 2024. Uses the workflow/agent distinction and starts with simpler patterns. Tooling details have evolved; no old vendor-specific tooling is adopted.

[^19]: Anthropic, [Demystifying Evals for AI Agents](https://www.anthropic.com/engineering/demystifying-evals-for-ai-agents), January 2026. Outcome-based evaluation, trials and complementary grading methods. First-party practitioner guidance, not a comparative validation of this product.

[^20]: W3C, [Web Content Accessibility Guidelines 2.2](https://www.w3.org/TR/WCAG22/), Recommendation edition dated 12 December 2024, and [Understanding Dragging Movements](https://www.w3.org/WAI/WCAG22/Understanding/dragging-movements.html). AA criteria include single-pointer non-drag alternatives; keyboard access alone is not that criterion. Applicable exceptions matter.

[^21]: Xin Luna Dong, Laure Berti-Equille and Divesh Srivastava, [Integrating Conflicting Data: The Role of Source Dependence](https://www.vldb.org/pvldb/vol2/vldb09-pvldb47.pdf), PVLDB 2(1), 2009. Original research on source copying/dependence. Its assumptions do not establish a universal real-world independence detector.

[^22]: OHCHR and UC Berkeley Human Rights Center, [Berkeley Protocol on Digital Open Source Investigations](https://humanrights.berkeley.edu/wp-content/uploads/archive/2024/02/Berkeley-Protocol.pdf), 2022 publication. Professional guidance on documentation, preservation, verification and analysis; selected concepts are adapted without claiming forensic certification.

### Supplementary typed decisions — owner direction, 21 September 2026

Jev can add message-kind, event-type-presence, claim-state, priority/translation/context, bounded message-relationship and reference-candidate suggestions. These are independently identified advisory outputs with exact inputs, receipts and provenance, available from the message Lineage view's advisory Jev branch. No suggestion discards/hides a message, replaces or skips baseline extraction, changes ordinary ordering, merges evidence/media or assigns coordinates. Uncertain/error outcomes retain the existing full path. The baseline remains authoritative for event-feed membership, exact claims and geographic publication. No normal-view classifier settings or additional panels are required. Model quality promotion remains a separate evaluated decision.

### Geographic context clarification — 21 September 2026

An exact place mention may share identities with unrelated facilities or an enclosing province. Intersect retained publisher aliases with the supported canonical name; do not replace an unmatched literal with a model-invented location. Literal capital qualifiers can restrict feature type. If the surviving alternatives are a province and its own localities, a uniquely matching accepted province polygon containing those localities may represent the common area. Explicit attached country context can retain a country area when the local feature remains unresolved. Both are coarse geographic results, not incident coordinates. Non-occurrence roles and unclear/denied/planned claims remain in the feed without occurrence geometry. Semantic incident clustering is a separate, unfinished capability.


Owner clarification, 21 September 2026: ordinary reading surfaces show source, timestamp/age and compact event types, using saved translations consistently. Country precedes the supported city/province name, retaining Approx./Area precision. Location selection counts messages in that shared point/area, not verified incidents. The specifically confirmed recurring sponsor footer is suppressed in presentation only; Original and exact provenance remain unchanged. Processing has no cumulative spending or daily request cutoff, per the owner’s explicit direction; no message is discarded or baseline-bypassed by Jev.

### Geographic area acceptance addendum — 23 September 2026

The existing exact-source, occurrence-state and A50 reference publication rules
also apply to selected marine regions. A named sea/strait may be an accepted
publisher polygon with area precision; it must never become a centroid incident
point or acquire a country merely from a model hint. Literal names and separately
reviewed aliases establish identity. Planned/denied/historical/issuer/destination
context cannot establish an occurrence area. Preserve full polygon holes, source
release IDs, bounds, rights, aliases and exact originals through replacement and
restore. Fixture: `app/tests/marine.rs` (explicitly fictional geometry), including
scope/history, multilingual names, ambiguity, corrupt-release rollback and backup.
`area_geometry::marine_contact_tests` distinguishes valid isolated tangency from
crossings, overlap, nested holes and disconnected interiors. This does not assert
complete global marine coverage or implement EEZ legal interpretation.


### Related-message delivery clarification — 23 September 2026

Ordinary message reading may expose one collapsed Related messages section with
source, timestamp/age and directional relationship labels. Selecting a peer opens
its exact retained revision; Original remains one menu action away. Suggestions
preserve every message and the common map/feed scope. Exact claim/readings and
publication history are required. Denials/corrections must remain distinguishable;
repeated reporting is not proof of independent confirmation. This increment supplies
pairwise context, not a transitive incident identity or automatic merge/split engine.
