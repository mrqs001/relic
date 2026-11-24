# Relic frontend

React, TypeScript and Vite provide one same-origin analyst workspace. The Rust application serves the production build. Source credentials and integration requests stay in Rust; no provider media, embeds, fonts or remote map tiles are fetched by this frontend.

## Build and run

From the project root:

```sh
npm ci --prefix web
npm run build --prefix web
```

Start the Rust application using the root application instructions. Its default address is `http://127.0.0.1:8790`. For frontend development, `npm run dev --prefix web` opens Vite on `127.0.0.1:5173` and proxies `/api` to Rust on port 8790. The runtime node version must satisfy Vite's declared engine requirement. The optional contract-check script below uses Node 22.18 or later for TypeScript stripping.

With an explicitly imported fictional validation workspace already running in local mode:

```sh
node web/scripts/check-contract.mjs http://127.0.0.1:8790
```

The checker validates actual Rust responses against the same runtime schemas used in the UI, shared query counts/generation and exact detail identity. It is read-only and rejects an empty or non-fictional workspace. Token-mode validation is performed through the actual interface, never by embedding a private token in this script.

## Implemented interface

- Explicit demo/import entry, five destinations and shared source, UTC time, publication, text and optional viewport scope.
- A persistent MapLibre map, exact colocated record chooser, virtualized evidence rows, URL/back/forward state and generation-checked queries. Camera movement alone changes no analytical scope. The selected loaded row remains mounted to preserve keyboard focus.
- Exact original text and Unicode passage citations, historical source-revision comparison, copied-origin explanation, unknown geography, original input bundle downloads when retained, and recorded lineage.
- Investigations, reasoned exact-revision pins, recoverable notes, explicit claim authoring and separate analyst assessments.
- Recoverable brief editor, acknowledged save state, new durable brief revisions, reopening and readable/manifest exports through the Rust application.
- Structured watch preview, explicit fresh baseline, grouped inbox, independent acknowledge/snooze/irrelevant review and pause/resume.
- React Flow lineage with readable initial root focus, selectable nodes and edges, fit/pan/zoom and an equivalent accessible list. Job state and real stop requests; snapshot backup plus separate restore verification.
- One sequence-aware SSE connection; bounded/coalesced query invalidation, prior-result labeling and session-aware cache clearing. Workspace token stays in transient form memory and is erased after login.

The map's local locator asset is copied unchanged from `reference/accepted/natural-earth/display/land.geojson`; the acceptance record there is authoritative. Natural Earth 1:110m v5.1.2 is a coarse locator, not facility geometry. Fictional coordinates are labeled illustrative. No inherited pack is automatically shown as accepted evidence.

## State and remaining boundaries

API responses are checked by the Zod schemas in `src/api.ts`. Rust owns eligibility, population counts, evidence identity, revisions, lineage and durable changes. Query data, URL/panel/draft state and the MapLibre instance have separate owners. Local draft recovery is not a durable save: conflicts stay visible, and rebasing recovered notes requires an explicit action.

Sources currently reports server capability metadata and provides explicit imports; its interface does not pretend that unimplemented live collection paths are enabled. Automatic contradiction detection is visibly unavailable in the watch builder; its current conditions are new observed origins and source corrections. The assistant, arbitrary known-at-time reconstruction, sensor controls, advanced recipe editing, richer timeline and media playback are not shipped by this slice. Historical selection uses actual retained publications.

The list loads stable pages of 100 records on demand, with server map totals and bounded map membership shown separately. Opening a map item outside the loaded list still resolves its exact evidence. MapLibre and React Flow use their native pan/zoom interactions; keyboard-operable panel sizing uses the workspace range control. On narrow screens the list is primary and an explicitly opened inspector overlays it.

A production build and schema pass do not establish accessibility or performance acceptance. The root task's `validation/` records contain actual UI walkthrough and workload evidence. `?benchmark=1` explicitly enables the separate synthetic rendering instrument and its test-only `window.__relicMap` hook; it neither imports evidence nor calls a provider.
