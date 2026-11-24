import { lazy, Suspense, useEffect, useMemo, useRef, useState } from "react";
import {
  useInfiniteQuery,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";
import { z } from "zod";
import { ArrowLeft, ChevronRight, Database,
  GitBranch, Globe2, List, MoreHorizontal,
  Plus, SlidersHorizontal, X } from "lucide-react";
import {
  api,
  ApiError,
  emptyScope,
  queryPreviewSchema,
  queryPageSchema,
  queryViewSchema,
  shapeSchema,
  workspaceSchema,
} from "./api";
import type { Scope, Workspace, QueryResult } from "./api";
import { readRoute, useRoute } from "./state";
import type { Destination } from "./state";
import type { ReactNode } from "react";
import SourcesPanel from "./components/SourcesPanel";
import { ProcessingPanel } from "./components/ProcessingPanel";
import PlaceSearch from "./components/PlaceSearch";
import type { MapFly } from "./components/MapPane";
import MapPane from "./components/MapPane";
import EvidenceList from "./components/EvidenceList";
import EvidenceInspector from "./components/EvidenceInspector";
import { Busy, ErrorMessage } from "./components/Common";
import {
  ImportButton,
} from "./components/CollectionPages";
const Benchmark = import.meta.env.DEV ? lazy(() => import("./Benchmark")) : null;
import Timeline from "./components/Timeline";
import { resolveTimeScope } from "./time-window";

const destinations = [
  { id: "sources", label: "Sources", icon: Database },
  { id: "workbench", label: "Processing", icon: GitBranch },
] as const;
type Drawer = "reports" | "layers" | "menu" | null;
type MapState = { center: [number, number]; zoom: number; bearing: number; pitch: number; bounds: [number, number, number, number]; projection: "globe" | "mercator"; basemap: "online" | "satellite" };
export default function App() {
  const [route, navigate] = useRoute(),
    client = useQueryClient();
  const routeRef = useRef(route);
  routeRef.current = route;
  const workspaceQuery = useQuery({
    queryKey: ["workspace"],
    queryFn: ({ signal }) => api("/workspace", workspaceSchema, { signal }),
    retry: (count, error) =>
      !(error instanceof ApiError && error.status === 401) && count < 1,
  });
  const sourceAttention = useQuery({ queryKey: ["source-attention"],
    queryFn: ({signal}) => api("/source-attention", attentionSchema, {signal}), refetchInterval:30000 });
  const [panel, setPanel] = useState<"evidence" | null>(route.evidence || route.messageKey ? "evidence" : null),
    [connected, setConnected] = useState(false),
    [drawer, setDrawer] = useState<Drawer>(null),
    [reportsVisible, setReportsVisible] = useState(true),
    [flyTo, setFlyTo] = useState<MapFly>(),
    [mapState, setMapState] = useState<MapState>();
  const drawerElement = useRef<HTMLElement>(null);
  const previousSurface = useRef(false);
  useEffect(() => {
    const opened = route.view !== "investigate" || !!drawer || !!panel;
    if (opened && drawer !== "menu") drawerElement.current?.focus({preventScroll:true});
    else if (!opened && previousSurface.current) document.querySelector<HTMLButtonElement>(".explore-brand")?.focus({preventScroll:true});
    previousSurface.current = opened;
  }, [drawer, panel, route.view]);
  useEffect(() => {
    const restorePanel = () => {
      const restored = readRoute();
      setPanel(restored.evidence || restored.messageKey ? "evidence" : null);
      setDrawer(null);
    };
    window.addEventListener("popstate", restorePanel);
    return () => window.removeEventListener("popstate", restorePanel);
  }, []);
  const [scopeError, setScopeError] = useState(route.invalidScope);
  useEffect(() => setScopeError(route.invalidScope), [route.invalidScope]);
  const messageRef = useQuery({
    queryKey: ["message-reference", route.messageKey], enabled: !!route.messageKey && !route.evidence,
    queryFn: ({ signal }) => api(`/messages/${encodeURIComponent(route.messageKey!)}`, z.object({ id: z.string(), revision: z.string() }), { signal }),
    retry: false,
  });
  useEffect(() => {
    if (messageRef.data && route.messageKey && !route.evidence)
      navigate({ evidence: messageRef.data.id, revision: messageRef.data.revision, messageKey: route.messageKey }, true);
  }, [messageRef.data, route.messageKey, route.evidence]);
  const messageKeyResolved = (id: string, revision: string, messageKey: string) => {
    const current = routeRef.current;
    if (current.evidence === id && current.revision === revision && current.messageKey !== messageKey)
      navigate({ messageKey }, true);
  };
  useEffect(() => {
    if (!route.invalidScope && !route.evidence && !route.messageKey) navigate({}, true);
  }, []);
  const workspace = workspaceQuery.data;
  // A rolling window ("last 24 h") is one scope: moving forward with the clock
  // refreshes it quietly instead of starting a new scope every 30 s. The exact
  // from/to used is resolved at fetch time and returned with the result.
  const queryScope = resolveTimeScope(route.scope, route.window, Date.now());
  const scopeKey = JSON.stringify({ scope: route.scope, window: route.window }),
    generationRef = useRef(0),
    generations = useRef(new Map<string, number>());
  // One stable generation per scope keeps its cache entry reusable: switching
  // back to a view shows its last result at once while it refreshes.
  const generation = useMemo(() => {
    const known = generations.current.get(scopeKey);
    if (known !== undefined) return known;
    const next = ++generationRef.current;
    generations.current.set(scopeKey, next);
    return next;
  }, [scopeKey]);
  const query = useInfiniteQuery({
    queryKey: ["evidence-query", scopeKey, generation],
    enabled: !!workspace && !workspace.empty,
    refetchInterval: route.window ? 30_000 : false,
    initialPageParam: null as { cursor: string; publication_id: string; snapshot: QueryResult } | null,
    queryFn: async ({ signal, pageParam }) => {
      if (pageParam) {
        // Later pages use the first page's exact scope, including a rolling window's from/to.
        const page=await api("/query/page",queryPageSchema,{method:"POST",signal,body:{scope:{...pageParam.snapshot.scope,publication_id:pageParam.publication_id},generation,limit:100,cursor:pageParam.cursor}});
        if(page.publication_id!==pageParam.publication_id || page.scope_revision!==pageParam.snapshot.scope_revision || page.generation!==generation) throw new Error("Feed page does not match the visible publication.");
        return {...pageParam.snapshot,...page};
      }
      const preview = await api("/query/preview", queryPreviewSchema, {
        method: "POST",
        body: {
          scope: resolveTimeScope(route.scope, route.window, Date.now()),
          generation,
          limit: 100,
        },
        signal,
      });
      // Edition-qualified area IDs identify immutable retained display shapes.
      // Query results are exposed only after all shapes match this publication.
      const features = [];
      for (let start=0;start<preview.map_areas.features.length;start+=4) {
        if (signal.aborted) throw new DOMException("Query superseded", "AbortError");
        features.push(...await Promise.all(preview.map_areas.features.slice(start,start+4).map(async feature => {
        if (feature.geometry) return feature;
        const shape = await client.fetchQuery({queryKey:["area-shape",feature.properties.area_id],
          staleTime:Infinity,gcTime:10*60_000,
          queryFn:({signal:shapeSignal})=>api(`/area-shape?id=${encodeURIComponent(feature.properties.area_id)}`,shapeSchema,{signal:shapeSignal})});
        if (shape.id!==feature.properties.area_id) throw new Error("Area shape identity changed.");
        return {...feature,geometry:shape.geometry};
        })));
      }
      const result = queryViewSchema.parse({...preview,map_areas:{...preview.map_areas,features}});
      if (result.generation !== generation)
        throw new ApiError(
          "This query was superseded. The visible publication has not been replaced.",
          "stale_generation",
          409,
        );
      return result;
    },
    getNextPageParam: (last) => last.next_cursor ? { cursor: last.next_cursor, publication_id: last.publication_id, snapshot: last } : undefined,
    placeholderData: (previous) => previous,
  });
  // Returning to the tab refreshes a rolling window (focus refetch is off app-wide).
  const refetchFeed = query.refetch;
  useEffect(() => {
    if (!route.window) return;
    const back = () => { if (document.visibilityState === "visible") void refetchFeed(); };
    document.addEventListener("visibilitychange", back);
    return () => document.removeEventListener("visibilitychange", back);
  }, [route.window, refetchFeed]);
  const result = query.data?.pages[0];
  const items = useMemo(
    () =>
      query.data?.pages
        .filter(
          (p) =>
            p.publication_id === result?.publication_id &&
            p.scope_revision === result.scope_revision &&
            p.generation === result.generation,
        )
        .flatMap((p) => p.items) ?? [],
    [query.data, result],
  );
  const updating =
    !!result && (query.isPlaceholderData || result.generation !== generation);
  // Stable across renders: the map's preview card depends on it.
  const sourceLabels = useMemo(() => Object.fromEntries((workspace?.sources ?? []).map(source => [source.id, source.label])), [workspace?.sources]);
  const exact = { id: route.evidence, revision: route.revision };
  const selectedReading = items.find(item => item.id === exact.id && item.revision === exact.revision)?.interpretation;
  const relatedReading = !updating && selectedReading?.related_key && selectedReading.related_count > 0 ? { key: selectedReading.related_key, count: selectedReading.related_count } : null;
  const select = (id: string, revision: string) => {
    navigate({
      view: "investigate",
      evidence: id,
      revision,
      messageKey: items.find(item => item.id === id && item.revision === revision)?.message_key ?? null,
      brief: null,
      investigation: null,
    });
    setDrawer(null);
    setPanel("evidence");
  };
  // Recall's flow: a feed row or map marker previews the message as a card on
  // the map and keeps the feed open; Open shows the full message. A feed row
  // also flies the map there. Messages without a place open in full.
  const preview = (id: string, revision: string, fromMap = false) => {
    const item = items.find(i => i.id === id && i.revision === revision);
    const site = item?.map_locations[0], area = item?.map_areas[0];
    // On a phone the feed covers the map, so a card there would be hidden.
    if ((!fromMap && !site && !area) || window.innerWidth <= 700) { select(id, revision); return; }
    navigate({ view: "investigate", evidence: id, revision, messageKey: item?.message_key ?? null, brief: null, investigation: null });
    setPanel(null);
    setDrawer("reports");
    if (fromMap) return;
    if (site) setFlyTo({ lng: site.geometry.coordinates[0], lat: site.geometry.coordinates[1], zoom: Math.max(7, mapState?.zoom ?? 0), nonce: Date.now() });
    else if (area) setFlyTo({ bounds: area.bounds, nonce: Date.now() });
  };
  const mediaRevision = (id: string, previous: string, revision: string) => {
    const current = routeRef.current;
    // An attachment completing after navigation must never reopen its message.
    if (current.evidence !== id || current.revision !== previous) return;
    navigate({ evidence: id, revision, scope: { ...current.scope, publication_id: null } });
    void client.invalidateQueries({ queryKey: ["evidence-query"] });
  };
  const openUpdatedMedia = (id: string, revision: string) => {
    // Explicit user navigation may leave a historical scope; passive companion
    // recovery only refreshes queries and never changes the selected message.
    navigate({ view: "investigate", evidence: id, revision, messageKey: null, scope: { ...routeRef.current.scope, publication_id: null } });
    setPanel("evidence");
    void client.invalidateQueries({ queryKey: ["evidence-query"] });
  };
  const showRelated = (related_key: string | null) => {
    navigate({ view: "investigate", evidence: null, revision: null, brief: null, scope: { ...route.scope, related_key } });
    setPanel(null); setDrawer("reports");
  };
  const importComplete = async () => {
    await client.invalidateQueries();
    navigate({ view: "investigate" });
    setPanel(null); setDrawer("reports");
  };
  const closeDrawer = () => { setPanel(null); setDrawer(null); navigate({ view: "investigate", evidence: null, revision: null, brief: null }); };
  const openDrawer = (next: Drawer) => {
    setDrawer(drawer === next ? null : next); setPanel(null);
    navigate({ view: "investigate", evidence: null, revision: null, brief: null });
  };
  const openDestination = (view: Destination) => {
    setDrawer(null); setPanel(null); navigate({ view, evidence: null, revision: null, brief: null });
  };
  useEffect(() => {
    const escape = (e: KeyboardEvent) => {
      if (e.key === "Escape" && !e.defaultPrevented && !document.querySelector("dialog[open]")) closeDrawer();
    };
    window.addEventListener("keydown", escape);
    return () => window.removeEventListener("keydown", escape);
  });
  const authorized = !!workspace;
  useEffect(() => {
    if (!authorized) return;
    const stream = new EventSource("/api/v1/events");
    let timer: ReturnType<typeof setTimeout> | undefined;
    let sequence = workspace?.sequence ?? 0;
    const kinds = new Set<string>();
    const supplementalInputs = new Set<string>();
    // The processing status reads the state of every retained message, and
    // collection and Jev emit events continuously while busy. Refresh it at
    // most every 2.5 s, always once more after the latest event.
    let processingTimer: ReturnType<typeof setTimeout> | undefined;
    let processingAt = 0;
    const refreshProcessing = () => {
      const wait = processingAt + 2500 - Date.now();
      if (wait > 0) {
        processingTimer ??= setTimeout(() => { processingTimer = undefined; refreshProcessing(); }, wait);
        return;
      }
      processingAt = Date.now();
      void client.invalidateQueries({ queryKey: ["processing"] });
      void client.invalidateQueries({ queryKey: ["processing-failures"] });
    };
    const refresh = (event: MessageEvent) => {
      let kind = "resync";
      if (event.type !== "resync") {
        try {
          const parsed = z
            .object({ sequence: z.number().int(), kind: z.string(), affected_ids:z.array(z.string()).default([]) })
            .parse(JSON.parse(event.data));
          if (parsed.sequence <= sequence) return;
          kind = parsed.sequence === sequence + 1 ? parsed.kind : "resync";
          sequence = parsed.sequence;
          if(parsed.kind === "supplemental") parsed.affected_ids.forEach(id=>supplementalInputs.add(id));
        } catch {
          kind = "resync";
        }
      }
      kinds.add(kind);
      if (timer) return;
      timer = setTimeout(() => {
        const changed = (...names: string[]) => kinds.has("resync") || names.some(name => kinds.has(name));
        // State/progress notifications do not change evidence membership. Only
        // a committed publication refreshes map/feed and an open reading.
        if (changed("publication", "analyst_work", "watch", "processing", "processing_state") || (kinds.has("job") && routeRef.current.view === "workbench"))
          void client.invalidateQueries({ queryKey: ["workspace"] });
        if (changed("publication")) {
          void client.invalidateQueries({ queryKey: ["evidence-query"] });
          void client.invalidateQueries({ queryKey: ["lineage"] });
        }
        if (changed("publication", "analyst_work") || supplementalInputs.has(`${routeRef.current.evidence}@${routeRef.current.revision}`))
          void client.invalidateQueries({ queryKey: ["evidence"] });
        if (changed("collection_settings", "connection", "collection_state", "collection")) {
          void client.invalidateQueries({ queryKey: ["subscriptions"] });
          void client.invalidateQueries({ queryKey: ["source-attention"] });
        }
        if (changed("connection"))
          void client.invalidateQueries({ queryKey: ["connections"] });
        if (changed("supplemental", "publication") || [...kinds].some(kind => kind.startsWith("processing")))
          refreshProcessing();
        if (changed("watch", "publication"))
          void client.invalidateQueries({ queryKey: ["alerts"] });
        kinds.clear();
        supplementalInputs.clear();
        timer = undefined;
      }, 750);
    };
    stream.onopen = () => setConnected(true);
    stream.onerror = () => setConnected(false);
    stream.onmessage = refresh;
    stream.addEventListener("change", refresh);
    stream.addEventListener("resync", refresh);
    return () => {
      stream.close();
      if (timer) clearTimeout(timer);
      if (processingTimer) clearTimeout(processingTimer);
      setConnected(false);
    };
  }, [authorized, client]);
  useEffect(() => {
    document.title = `${destinations.find((d) => d.id === route.view)?.label || "Explore"} · Relic`;
  }, [route.view]);
  const managementView = route.view === "sources" || route.view === "workbench";
  const activeDrawer = (!managementView && route.view !== "investigate") || !!drawer || !!panel;
  const drawerTitle = panel === "evidence" ? "Message" : drawer === "reports" ? "Feed" : drawer === "layers" ? "Filter feed" : "Workspace";
  const filtered = !!route.scope.event_type || !!route.scope.related_key || route.scope.content_view === "signals" || !!route.scope.query || !!route.scope.bbox || !!route.scope.publication_id || !!route.scope.source_ids.length || !route.scope.include_unlocated;
  return (
    <div className="explore-shell">
      <a className="skip-link" href="#map-tools">Skip to map tools</a>
      <main id="main-content" className="globe-workspace" aria-label="Relic geographic workspace">
        <header className="explore-header" id="map-tools">
          <button className="explore-brand" aria-label="Relic · close panels" onClick={closeDrawer}><Globe2 size={25} strokeWidth={1.15}/><span>Relic</span></button>
          <PlaceSearch onPlace={p => { setFlyTo({ lng: p.longitude, lat: p.latitude, zoom: p.kind === "city" ? Math.max(9.5, p.zoom) : p.zoom, nonce: Date.now() }); }}
            onReports={query => { navigate({ view: "investigate", scope: { ...route.scope, query } }); setDrawer("reports"); setPanel(null); }}/>
          <nav className="explore-nav" aria-label="Map workspace tools">
            <button aria-label="Feed" aria-pressed={drawer === "reports" || panel === "evidence"} onClick={() => openDrawer("reports")}><List size={17}/><span>Feed</span>{filtered && <i className="filter-dot"/>}</button>
            <button aria-label="Sources" aria-pressed={route.view === "sources"} onClick={() => route.view === "sources" ? closeDrawer() : openDestination("sources")}><Database size={17}/><span>Sources</span></button>
            <span className="nav-divider"/>
            <button aria-label="More tools" aria-pressed={drawer === "menu"} onClick={() => openDrawer("menu")}><MoreHorizontal size={21}/></button>
          </nav>
        </header>
        <MapPane historical={!!route.scope.publication_id} historicalAt={route.scope.publication_id ? workspace?.publications.find(p => p.id === route.scope.publication_id)?.created_at ?? null : null} result={result} sourceLabels={sourceLabels} selected={exact} onSelect={(id, revision) => preview(id, revision, true)}
          previewing={!!route.evidence && !panel} previewItem={items.find(item => item.id === exact.id && item.revision === exact.revision)}
          onOpen={() => { setDrawer(null); setPanel("evidence"); }} onClosePreview={() => navigate({ evidence: null, revision: null })}
          onScope={bbox => navigate({ scope: { ...route.scope, bbox } })}
          fictional={workspace?.fictional ?? false} updating={updating}
          flyTo={flyTo} reportsVisible={reportsVisible} onMapState={setMapState}/>
        {filtered && !activeDrawer && <div className="active-scope"><button onClick={() => openDrawer(route.scope.related_key ? "reports" : "layers")}><SlidersHorizontal size={12}/>{route.scope.related_key ? "Related reporting" : route.scope.query ? `“${route.scope.query}”` : route.scope.publication_id ? "Historical snapshot" : route.scope.bbox ? "Map area" : route.scope.content_view === "signals" ? "Signals" : "Filtered reports"}</button><button aria-label={route.scope.related_key ? "Clear related reporting" : "Clear all report filters"} onClick={() => navigate({ scope: route.scope.related_key ? { ...route.scope, related_key: null } : emptyScope, window: route.scope.related_key ? route.window : null })}><X size={12}/></button></div>}
        {scopeError && <div className="floating-error" role="alert">Invalid link filter. <button className="text-button" onClick={() => { navigate({scope:emptyScope,window:null},true); setScopeError(false); }}>Reset filters</button></div>}
        {workspaceQuery.error && authorized && <div className="floating-error"><ErrorMessage error={workspaceQuery.error}/></div>}
        {query.error && !drawer && !panel && <div className="floating-error"><ErrorMessage error={query.error}/></div>}
        { !workspace ? <div className="connection-toast"><ErrorMessage error={workspaceQuery.error}/>{workspaceQuery.isLoading ? <Busy>Connecting workspace…</Busy> : <button className="button" onClick={() => workspaceQuery.refetch()}>Reconnect workspace</button>}</div>
        : <>
          {!workspace.empty && <Timeline window={route.window} scope={result?.scope ?? queryScope} data={result?.timeline} busy={updating || !result}
            onWindow={window => navigate({ window, scope: { ...route.scope, from: null, to: null } })}
            onInterval={(from, to) => navigate({ window: null, scope: { ...route.scope, from, to } })}
            onBasis={time_basis => navigate({ scope: { ...route.scope, time_basis } })}/>}
          {drawer === "menu" ? <div className="workspace-menu floating-surface" aria-label="More tools menu">
            <button onClick={() => openDrawer("layers")}><SlidersHorizontal size={17}/><span>Filter feed</span><ChevronRight size={14}/></button>
            {destinations.filter(d => d.id !== "sources").map(({id,label,icon:Icon}) => <button key={id} onClick={() => openDestination(id)}><Icon size={17}/><span>{label}</span><ChevronRight size={14}/></button>)}
            <small><i className={connected ? "online-dot" : "offline-dot"}/>{connected ? "Local workspace connected" : "Updates reconnecting"}</small>
          </div> : activeDrawer && <section className={`explore-drawer floating-surface ${drawer === "layers" ? "filter-drawer" : ""} ${route.view === "workbench" ? "wide-drawer" : ""} ${route.view === "library" || route.view === "sources" || route.view === "watch" ? "collection-drawer" : ""}`} aria-label={drawerTitle} ref={drawerElement} tabIndex={-1}>
            <div className="drawer-heading">
              <div>{panel && <button className="icon-button" aria-label="Back to feed" onClick={() => { setPanel(null); setDrawer("reports"); navigate({ evidence: null, revision: null }); }}><ArrowLeft size={16}/></button>}<h1>{drawerTitle}</h1>{drawer === "reports" && <span>{result?.total ?? 0}</span>}</div>
              <div>{drawer === "reports" && <button className="icon-button" aria-label="Filter feed" onClick={() => openDrawer("layers")}><SlidersHorizontal size={16}/></button>}<button className="icon-button" aria-label="Close panel" onClick={closeDrawer}><X size={17}/></button></div>
            </div>
            {drawer === "reports" && (workspace.empty ? <div className="drawer-section empty-import"><h2>No reports yet</h2><p>Add a source bundle or try the fictional demo.</p><ImportButton onImported={importComplete}/><ImportButton demo onImported={importComplete}/><small>Demo reports are fictional and stay separate from real evidence.</small></div> : <EvidenceList items={items} result={result} selected={exact} onSelect={(id, revision) => preview(id, revision)} updating={updating} error={query.error} hasMore={!!query.hasNextPage} loadMore={() => { void query.fetchNextPage(); }} fetchingMore={query.isFetchingNextPage} contentView={route.scope.content_view} onContentView={(content_view, clearType) => navigate({ scope: { ...route.scope, content_view, event_type: clearType ? null : route.scope.event_type } })} eventType={route.scope.event_type} onEventType={event_type => navigate({ scope: { ...route.scope, event_type } })} relatedKey={route.scope.related_key} onRelated={showRelated}/>)}
            {drawer === "layers" && <div className="drawer-scroll">
              <div className="drawer-section"><h2>Area</h2><div className="feed-area"><button className="button" aria-pressed={!route.scope.bbox} onClick={() => navigate({ scope: { ...route.scope, bbox: null } })}>Worldwide</button><button className="button" aria-pressed={!!route.scope.bbox} disabled={!mapState} onClick={() => mapState && navigate({ scope: { ...route.scope, bbox: mapState.bounds } })}>This map area</button></div>{filtered && <button className="text-button" onClick={() => navigate({ scope: emptyScope, window: null })}>Clear filters</button>}</div>
              <details className="advanced-filters"><summary>Advanced filters{filtered ? " · active filters" : ""}</summary><ScopeBar scope={route.scope} workspace={workspace} onChange={scope => navigate({ scope })} updating={updating} publication={result?.publication_id || null}/><div className="drawer-section"><label className="layer-toggle"><span>Show reports on map</span><input type="checkbox" checked={reportsVisible} onChange={e => setReportsVisible(e.target.checked)}/></label></div></details>
            </div>}
            {panel === "evidence" && messageRef.isError && <ErrorMessage error={messageRef.error}/>}
            {panel === "evidence" && messageRef.isLoading && <Busy/>}
            {panel === "evidence" && <EvidenceInspector scope={{...route.scope,publication_id:result?.publication_id ?? route.scope.publication_id}} id={route.evidence} revision={route.revision} snapshot={result?.publication_id ?? route.scope.publication_id} autoResolveMedia={!route.scope.publication_id} onMediaRevision={mediaRevision} onOpenUpdatedMedia={openUpdatedMedia} onMessageKey={messageKeyResolved} related={relatedReading} onRelated={showRelated} onLocate={coordinates => setFlyTo(coordinates.length === 4 ? {bounds:coordinates,nonce:Date.now()} : {lng:coordinates[0],lat:coordinates[1],zoom:6,nonce:Date.now()})} onSelect={select}/>}
          </section>}
          <div className="map-status">{sourceAttention.data && sourceAttention.data.count>0 && <AttentionButton attention={sourceAttention.data} onSources={() => navigate({...route,view:"sources"})}/>}<div className="map-status-readout" role="status"><i className={connected ? "online-dot" : "offline-dot"}/>{workspace.fictional && <span className="fixture-indicator">Fictional</span>}<span>{updating ? "Updating filters…" : result ? `${result.total} ${result.total === 1 ? "message" : "messages"}` : workspace.empty ? "No reports yet" : query.error ? "Reports unavailable" : "Loading reports…"}</span>{!!(result?.unread || result?.unlocated) && <span className="status-unlocated">{result?.unread ? `${result.unread} unread` : `${result?.unlocated} unlocated`}</span>}{route.scope.publication_id && <span className="history-indicator">Historical</span>}</div>{workspace.empty && <button onClick={() => openDrawer("reports")}>Import <Plus size={12}/></button>}{!reportsVisible && <button onClick={() => setReportsVisible(true)}>Reports hidden <X size={12}/></button>}</div>
        </>}
      </main>
      {workspace && authorized && managementView && <WorkspaceModal title={route.view === "sources" ? "Sources" : "Processing"} onClose={closeDrawer}>{route.view === "sources" ? <SourcesPanel workspace={workspace}/> : <ProcessingPanel workspace={workspace}/>}</WorkspaceModal>}
      {Benchmark && new URLSearchParams(location.search).get("benchmark") === "1" && <Suspense fallback={null}><Benchmark/></Suspense>}
    </div>
  );
}

function ScopeBar({
  scope,
  workspace,
  onChange,
  updating,
  publication,
}: {
  scope: Scope;
  workspace: Workspace;
  onChange: (scope: Scope) => void;
  updating: boolean;
  publication: string | null;
}) {
  const [search, setSearch] = useState(scope.query),
    timer = useRef<ReturnType<typeof setTimeout> | null>(null),
    currentScope = useRef(scope),
    change = useRef(onChange);
  currentScope.current = scope;
  change.current = onChange;
  useEffect(() => {
    setSearch(scope.query);
  }, [scope.query]);
  useEffect(
    () => () => {
      if (timer.current) clearTimeout(timer.current);
    },
    [],
  );
  return <section className="scope-bar drawer-section" aria-label="Shared evidence scope">
    <h2>Report filters</h2>
    <label>Text<input aria-label="Search evidence" value={search} placeholder="Search retained text…" onChange={e => {
      const value = e.target.value; setSearch(value);
      if (timer.current) clearTimeout(timer.current);
      timer.current = setTimeout(() => change.current({ ...currentScope.current, query:value }),250);
    }}/></label>
    <fieldset><legend>Sources</legend>{workspace.sources.map(source => <label className="checkbox-label" key={source.id}><input type="checkbox" checked={!scope.source_ids.length || scope.source_ids.includes(source.id)} onChange={e => {
      const selected = scope.source_ids.length ? scope.source_ids : workspace.sources.map(s => s.id);
      const next = e.target.checked ? [...selected,source.id] : selected.filter(id => id !== source.id);
      onChange({...scope,source_ids:next.length ? next : selected});
    }}/>{source.label.replace("Fictional ","")}</label>)}<small>At least one source stays selected.</small></fieldset>
    <label>Publication snapshot<select aria-label="Publication snapshot" value={scope.publication_id || ""} onChange={e => onChange({...scope,publication_id:e.target.value || null})}><option value="">Latest complete publication</option>{scope.publication_id && !workspace.publications.some(p => p.id === scope.publication_id) && <option value={scope.publication_id}>Selected snapshot · {scope.publication_id}</option>}{workspace.publications.filter(p => ["complete","frozen"].includes(p.status)).map(p => <option key={p.id} value={p.id}>{p.label}</option>)}</select>{workspace.publications_truncated && <small>Recent {workspace.publications.length} of {workspace.publications_total ?? "all"} snapshots. Saved selections keep their exact publication.</small>}</label>
    <label className="checkbox-label"><input type="checkbox" checked={scope.include_unlocated} onChange={e => onChange({...scope,include_unlocated:e.target.checked})}/>Include unlocated reports</label>
    {scope.bbox && <button className="text-button" onClick={() => onChange({...scope,bbox:null})}>Clear map area <X size={12}/></button>}
    <small title={publication || "No publication"}>{updating ? "Updating · previous publication visible" : scope.publication_id ? "Historical publication" : "Latest complete publication"}</small>
  </section>;
}
const attentionSchema = z.object({
  count: z.number(),
  sources: z.number().optional(),
  layers: z.array(z.object({ id: z.string(), label: z.string(), state: z.string(), detail: z.string().nullish(), since: z.string().nullish() })).default([]),
});
type Attention = z.infer<typeof attentionSchema>;
/** Sources and live layers needing attention. It opens Sources when any
 *  source is affected, otherwise the Layers panel, where each layer says why. */
function AttentionButton({ attention, onSources }: { attention: Attention; onSources: () => void }) {
  const layers = attention.layers.length;
  const sources = attention.sources ?? Math.max(0, attention.count - layers);
  const parts = [sources ? `${sources} source${sources === 1 ? "" : "s"}` : "", layers ? `${layers} layer${layers === 1 ? "" : "s"}` : ""].filter(Boolean);
  const label = `${parts.join(" and ")} ${sources + layers === 1 ? "needs" : "need"} attention`;
  const details = attention.layers.map(l => `${l.label}: ${l.detail ?? l.state}`).join("\n");
  return <button title={details || undefined} onClick={() => sources ? onSources() : window.dispatchEvent(new CustomEvent("relic:open-layers"))}>{label}</button>;
}
function WorkspaceModal({ title, onClose, children }: { title: string; onClose: () => void; children: ReactNode }) {
  const dialog = useRef<HTMLDialogElement>(null);
  useEffect(() => { dialog.current?.showModal(); }, []);
  return <dialog ref={dialog} className="workspace-dialog" aria-label={title} onCancel={onClose}>
    <header className="workspace-dialog-heading"><h1>{title}</h1><button className="icon-button" aria-label={`Close ${title.toLowerCase()}`} onClick={onClose}><X size={18}/></button></header>
    <div className="workspace-dialog-body">{children}</div>
  </dialog>;
}
