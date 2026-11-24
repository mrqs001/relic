import { useEffect, useMemo, useRef, useState } from 'react';
import { useQuery } from '@tanstack/react-query';
import { z } from 'zod';
import { api, eventTypes } from '../api';
import maplibregl from 'maplibre-gl';
import type { Feature, FeatureCollection, Geometry, Point, Position } from 'geojson';
import { ChevronDown, Compass, Globe2, LocateFixed, Map as FlatMap, Minus, Plus, RotateCcw, WifiOff, X } from 'lucide-react';
import type { EventType, EvidencePreview, QueryResult } from '../api';
import { eventColors, eventGroup, eventGroups, groupColors, EventLabel, primaryEvent } from './EventLabel';
import { cleanReadingText, feedPlace, placeLabel, readingPreview } from './reading-display';
import MapPreview from './MapPreview';
import type { PreviewContent } from './MapPreview';
import { MessageTime } from './MessageTime';
import { CAMERA_KEY, homeCamera, coordinateLabel, loadingStyle, normalizeViewportBounds, onlineStyle, projectionSpec, readCamera, SATELLITE_ORIGIN } from './map-style';
import type { BasemapMode, ProjectionMode } from './map-style';
import 'maplibre-gl/dist/maplibre-gl.css';
import './map-controls.css';
import LiveLayers, { liveFeatureNear } from './LiveLayers';

declare global { interface Window { __relicMap?: maplibregl.Map } }
export type MapViewState = { center: [number, number]; zoom: number; bearing: number; pitch: number; bounds: [number, number, number, number]; projection: ProjectionMode; basemap: BasemapMode };
const choiceSchema = z.object({id:z.string(),revision:z.string(),title:z.string(),source:z.string(),published_at:z.string().nullish(),acquired_at:z.string().optional(),processed:z.boolean().optional(),location:z.string().optional(),site_key:z.string().optional(),events:z.array(z.object({type:z.enum(eventTypes),state:z.enum(["reported","denied","planned","historical","unclear"])})).default([])});
type Choice = z.infer<typeof choiceSchema>;
function mapChoice(properties: Record<string, unknown>): Choice {
  return choiceSchema.parse({...properties,id:properties.evidence_id,events:typeof properties.events === "string" ? JSON.parse(properties.events) : properties.events});
}
export type MapFly = { lng: number; lat: number; zoom?: number; nonce: number } | { bounds: [number, number, number, number]; nonce: number };
type Props = {
  result?: QueryResult;
  sourceLabels: Record<string, string>;
  selected: { id: string | null; revision: string | null };
  onSelect: (id: string, revision: string) => void;
  onScope: (bbox: [number, number, number, number]) => void;
  fictional: boolean;
  updating: boolean;
  flyTo?: MapFly;
  reportsVisible?: boolean;
  onMapState?: (state: MapViewState) => void;
  /** A chosen historical snapshot: layers show its time where history exists. */
  historical?: boolean;
  historicalAt?: string | null;
  /** The selected message is previewed on the map (the feed stays open). */
  previewing?: boolean;
  /** Its feed row, when loaded: the richest text and place for the card. */
  previewItem?: EvidencePreview;
  onOpen?: () => void;
  onClosePreview?: () => void;
};
type MapFeature = QueryResult["map"]["features"][number];
/** One marker per linked event at each site: the newest report stands for the
 *  event and every collapsed report stays reachable from it (`members`). */
function collapseEvents(map: QueryResult["map"] | undefined, members: Map<string, MapFeature[]>): QueryResult["map"] {
  members.clear();
  if (!map) return { type: "FeatureCollection", features: [] };
  const time = (f: MapFeature) => String(f.properties.published_at ?? f.properties.acquired_at ?? "");
  const leads = new Map<string, MapFeature>();
  const features: MapFeature[] = [];
  for (const f of map.features) {
    const group = f.properties.event_group;
    if (typeof group !== "string") { features.push(f); continue; }
    const key = `${group}|${String(f.properties.site_key ?? f.geometry.coordinates.join(","))}`;
    const list = members.get(key);
    if (list) {
      list.push(f);
      const lead = leads.get(key)!;
      if (time(f) > time(lead)) { lead.properties = { ...f.properties, event_key: key }; lead.geometry = f.geometry; }
      lead.properties.site_reports = list.length;
      continue;
    }
    members.set(key, [f]);
    const lead = { ...f, properties: { ...f.properties, event_key: key, site_reports: 1 } };
    leads.set(key, lead);
    features.push(lead);
  }
  return { type: "FeatureCollection", features };
}
// Clusters and region markers take the colour of their most common event
// group, so the map shows what is happening, not only how much.
const NEUTRAL = '#6f9aa6';
const legendNames: Record<string, string> = { 'Military activity': 'Military', 'Politics and diplomacy': 'Politics', 'Civil affairs': 'Civil' };
const groupTypes = eventGroups.map(group => Object.keys(eventGroup).filter(type => eventGroup[type as keyof typeof eventGroup] === group));
// Region markers carry their own per-group counts (n0…) and message total; a
// point counts its reports. Clusters add both, so their number is reports.
const clusterProperties = {
  ...Object.fromEntries(groupTypes.map((types, i) => [`g${i}`, ['+', ['coalesce', ['get', `n${i}`], ['case', ['in', ['get', 'event_type'], ['literal', types]], ['coalesce', ['get', 'site_reports'], 1], 0]]]])),
  reports: ['+', ['coalesce', ['get', 'message_count'], ['get', 'site_reports'], 1]],
} as Record<string, maplibregl.ExpressionSpecification>;
const clusterColor = ['case', ...eventGroups.flatMap((group, i) => [['all', ['>', ['get', `g${i}`], 0], ...eventGroups.flatMap((_, j) => j === i ? [] : [['>=', ['get', `g${i}`], ['get', `g${j}`]]])], groupColors[group]]), NEUTRAL] as unknown as maplibregl.ExpressionSpecification;
/** Messages per event group (each message by its first reported event). */
function groupCounts(choices: QueryResult["map_areas"]["features"][number]["properties"]["choices"]) {
  const counts = eventGroups.map(() => 0);
  for (const choice of choices) {
    const type = choice.events.find(event => event.state === 'reported')?.type;
    const index = type ? eventGroups.indexOf(eventGroup[type]) : -1;
    if (index >= 0) counts[index]++;
  }
  return counts;
}
function countsColor(counts: number[]) {
  const best = counts.reduce((top, count, i) => count > counts[top] ? i : top, 0);
  return counts[best] > 0 ? groupColors[eventGroups[best]] : NEUTRAL;
}
function ringArea(ring: Position[]) {
  let area = 0;
  for (let i = 0, j = ring.length - 1; i < ring.length; j = i++) area += (ring[j][0] + ring[i][0]) * (ring[j][1] - ring[i][1]);
  return area / 2;
}
function inRing([x, y]: Position, ring: Position[]) {
  let inside = false;
  for (let i = 0, j = ring.length - 1; i < ring.length; j = i++) {
    const [xi, yi] = ring[i], [xj, yj] = ring[j];
    if ((yi > y) !== (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi) inside = !inside;
  }
  return inside;
}
/** A point inside a region for its marker: the largest polygon's centroid, or
 *  the middle of its widest span when the centroid falls outside (a crescent). */
function labelPoint(geometry: Geometry | null | undefined, [w, s, e, n]: [number, number, number, number]): [number, number] {
  const fallback: [number, number] = [w <= e ? (w + e) / 2 : (((w + e + 360) / 2 + 180) % 360) - 180, (s + n) / 2];
  const polygons = geometry?.type === 'Polygon' ? [geometry.coordinates] : geometry?.type === 'MultiPolygon' ? geometry.coordinates : [];
  if (!polygons.length) return fallback;
  const polygon = polygons.reduce((a, b) => Math.abs(ringArea(b[0])) > Math.abs(ringArea(a[0])) ? b : a);
  const outer = polygon[0];
  const inside = (p: Position) => inRing(p, outer) && !polygon.slice(1).some(hole => inRing(p, hole));
  let cx = 0, cy = 0, a = 0;
  for (let i = 0, j = outer.length - 1; i < outer.length; j = i++) {
    const f = outer[j][0] * outer[i][1] - outer[i][0] * outer[j][1];
    cx += (outer[j][0] + outer[i][0]) * f; cy += (outer[j][1] + outer[i][1]) * f; a += f;
  }
  if (a !== 0) { const c: [number, number] = [cx / (3 * a), cy / (3 * a)]; if (inside(c)) return c; }
  let low = Infinity, high = -Infinity;
  for (const [, y] of outer) { if (y < low) low = y; if (y > high) high = y; }
  const y = (low + high) / 2, xs: number[] = [];
  for (const ring of polygon) for (let i = 0, j = ring.length - 1; i < ring.length; j = i++) {
    const [xi, yi] = ring[i], [xj, yj] = ring[j];
    if ((yi > y) !== (yj > y)) xs.push((xj - xi) * (y - yi) / (yj - yi) + xi);
  }
  xs.sort((p, q) => p - q);
  let best: [number, number] | null = null, width = 0;
  for (let i = 0; i + 1 < xs.length; i += 2) if (xs[i + 1] - xs[i] > width) { width = xs[i + 1] - xs[i]; best = [(xs[i] + xs[i + 1]) / 2, y]; }
  return best ?? fallback;
}
/** Region reports as one marker per region (outlines only on hover or
 *  selection), each polygon carrying its marker colour. */
function regionLayers(areas: QueryResult["map_areas"] | undefined): { polygons: FeatureCollection; anchors: FeatureCollection } {
  const polygons: Feature[] = [], anchors: Feature[] = [];
  for (const area of areas?.features ?? []) {
    const counts = groupCounts(area.properties.choices), color = countsColor(counts);
    // Choices hold at most 100 newest messages; scale to the whole region.
    const scale = area.properties.choices.length ? area.properties.message_count / area.properties.choices.length : 0;
    polygons.push({ ...area, properties: { ...area.properties, color } } as Feature);
    anchors.push({ type: 'Feature', geometry: { type: 'Point', coordinates: labelPoint(area.geometry as Geometry | null, area.properties.bounds) }, properties: { area_id: area.properties.area_id, label: area.properties.label, level: area.properties.level, country_code: area.properties.country_code, message_count: area.properties.message_count, color, ...Object.fromEntries(counts.map((count, i) => [`n${i}`, count * scale])) } });
  }
  return { polygons: { type: 'FeatureCollection', features: polygons }, anchors: { type: 'FeatureCollection', features: anchors } };
}
/** Point reports and region markers in one clustered source. */
function reportFeatures(result: QueryResult | undefined, members: Map<string, MapFeature[]>) {
  return { type: 'FeatureCollection', features: [...collapseEvents(result?.map, members).features, ...regionLayers(result?.map_areas).anchors.features] } as FeatureCollection;
}
/** What a reference area is: a country, province, OSM facility or OSM place. */
function areaKind(level: string) {
  return level === "ADM0" ? "country" : level === "FACILITY" ? "facility" : level === "PLACE" ? "place" : level === "MARINE" ? "sea" : "province";
}
function areaChoices(areas: QueryResult["map_areas"]["features"]): Choice[] {
  return [...new Map([...areas].sort((a,b) => a.properties.level.localeCompare(b.properties.level)).flatMap(a => a.properties.choices.map(c => ({...c,location:`${placeLabel({id:a.properties.area_id,...a.properties})} · ${areaKind(a.properties.level)} area`}))).map(c => [`${c.id}@${c.revision}`, c])).values()];
}
const eventColor: maplibregl.ExpressionSpecification = ['match', ['get', 'event_type'], 'strike', eventColors.strike, ...Object.entries(eventColors).filter(([key]) => key !== 'strike').flat(), '#72e0e5'];
const empty: FeatureCollection = { type: 'FeatureCollection', features: [] };
const reportLayers = ['area-fill', 'area-outline', 'selected-area-fill', 'selected-area-outline', 'area-anchor-halo', 'area-anchor', 'area-anchor-count', 'evidence-halo', 'evidence-cluster-halo', 'evidence-clusters', 'evidence-points', 'evidence-counts', 'selection-ring', 'selection-core'];
const reduced = () => matchMedia('(prefers-reduced-motion: reduce)').matches;
const cameraDuration = (ms: number) => reduced() ? 0 : ms;
// Fit into the visible map, measuring the existing overlays at the moment of
// an explicit camera action. Opening/closing a panel never moves the camera.
function visiblePadding(map: maplibregl.Map) {
  const box = map.getContainer().getBoundingClientRect();
  const drawer = document.querySelector('.explore-drawer')?.getBoundingClientRect();
  const timeline = document.querySelector('.map-timeline')?.getBoundingClientRect();
  const right = drawer && drawer.width < box.width * .7 ? box.right - drawer.left + 32 : 48;
  const bottom = timeline && timeline.top > box.top + box.height * .5 ? box.bottom - timeline.top + 24 : 96;
  return { left:Math.min(96,box.width * .15),right:Math.min(right,box.width * .55),top:Math.min(100,box.height * .2),bottom:Math.min(bottom,box.height * .3) };
}

export default function MapPane({ result, sourceLabels, selected, onSelect, fictional, updating, flyTo, reportsVisible = true, onMapState, historical = false, historicalAt = null, previewing = false, previewItem, onOpen, onClosePreview }: Props) {
  // Where the selected message is drawn (its point, else the marker of a region
  // listing it) and what its card says.
  const preview = useMemo((): { at: [number, number]; content: PreviewContent } | null => {
    if (!previewing || !selected.id || !result) return null;
    const point = result.map.features.find(f => f.properties.evidence_id === selected.id && f.properties.revision === selected.revision);
    const area = point ? undefined : result.map_areas.features.find(a => a.properties.choices.some(c => c.id === selected.id && c.revision === selected.revision));
    if (!point && !area) return null;
    const at = point ? point.geometry.coordinates as [number, number] : labelPoint(area!.geometry as Geometry | null, area!.properties.bounds);
    if (previewItem) {
      const primary = primaryEvent(previewItem.interpretation).primary;
      return { at, content: {
        source: sourceLabels[previewItem.source] || previewItem.source_label,
        time: previewItem.published_at || previewItem.acquired_at || null,
        place: feedPlace(previewItem),
        event: primary ? { type: primary.type, possible: false } : previewItem.possible_event ? { type: previewItem.possible_event.type, possible: true } : null,
        text: readingPreview(previewItem),
      } };
    }
    const props = point ? point.properties as Record<string, unknown> : area!.properties.choices.find(c => c.id === selected.id && c.revision === selected.revision) as Record<string, unknown>;
    const events = (typeof props.events === 'string' ? JSON.parse(props.events) : props.events ?? []) as { type: EventType; state: string }[];
    const reported = events.find(e => e.state === 'reported') ?? events[0];
    return { at, content: {
      source: sourceLabels[String(props.source)] || 'Message',
      time: String(props.published_at || props.acquired_at || '') || null,
      place: area ? { place: placeLabel({ id: area.properties.area_id, ...area.properties }), country: null } : null,
      event: reported ? { type: reported.type, possible: false } : null,
      text: cleanReadingText(String(props.title || '')) || 'Media-only message',
    } };
  }, [previewing, selected.id, selected.revision, result, previewItem, sourceLabels]);
  const areaSelection = useQuery({
    queryKey: ['area-selection', selected.id, selected.revision, result?.scope_revision],
    enabled: !!selected.id && !!selected.revision && !!result?.map_areas.features.length,
    queryFn: ({ signal }) => api('/area-selection', z.object({ ids: z.array(z.string()) }), { method: 'POST', body: { scope: result?.scope, id: selected.id, revision: selected.revision }, signal }),
  });
  const selectedAreas = areaSelection.data?.ids ?? [];
  const initial = useRef(readCamera());
  const openingFit = useRef(!initial.current.restored);
  const host = useRef<HTMLDivElement>(null), map = useRef<maplibregl.Map | null>(null), coordinateNode = useRef<HTMLSpanElement>(null), compassNode = useRef<SVGSVGElement>(null);
  const latest = useRef({ result, sourceLabels, selected, selectedAreas, onSelect, reportsVisible, onMapState, flyTo });
  // Collapsed event markers → their reports, rebuilt whenever report data changes.
  const eventMembers = useRef(new Map<string, MapFeature[]>()).current;
  latest.current = { result, sourceLabels, selected, selectedAreas, onSelect, reportsVisible, onMapState, flyTo };
  const [projection, setProjection] = useState<ProjectionMode>(initial.current.projection);
  const projectionRef = useRef(projection);
  const [basemap, setBasemap] = useState<BasemapMode>(initial.current.basemap);
  const basemapRef = useRef(basemap);
  const switchBasemapRef = useRef<(mode: BasemapMode) => void>(() => {});
  const detailedStyleLoaded = useRef(false);
  const [ready, setReady] = useState(false), [styleMenu, setStyleMenu] = useState(false), [choices, setChoices] = useState<Choice[]>([]), [choiceTotal, setChoiceTotal] = useState(0), [loading, setLoading] = useState(true);
  const chosenAreas = useRef<string[]>([]);
  const menuState = useRef({ styleOpen: false, choicesOpen: false });
  menuState.current = { styleOpen: styleMenu, choicesOpen: choices.length > 0 };
  const [choicePlace, setChoicePlace] = useState("At this location");
  const [notice, setNotice] = useState(''), [fatal, setFatal] = useState('');
  const retryMapRef = useRef<() => void>(() => {});
  const applySelectionRef = useRef<() => void>(() => {}), applyReportsRef = useRef<() => void>(() => {}), flyRef = useRef<() => void>(() => {}), publishCameraRef = useRef<() => void>(() => {});

  useEffect(() => {
    if (!host.current) return;
    let disposed = false, styleGeneration = 0, styleRequest: AbortController | null = null, retryTimeout: ReturnType<typeof setTimeout> | undefined;
    let lastFlyNonce: number | null = null, cursorFrame = 0, latestPointer: maplibregl.MapMouseEvent | null = null, clusterRequest = 0;
    const countImages = new Set<string>();
    let m: maplibregl.Map;
    try {
      m = new maplibregl.Map({
        container: host.current, style: loadingStyle(initial.current.projection),
        center: initial.current.center, zoom: initial.current.zoom, bearing: initial.current.bearing, pitch: initial.current.pitch,
        minZoom: -0.75, maxZoom: 19, maxPitch: 65, attributionControl: false, renderWorldCopies: false,
        fadeDuration: 220,
        transformRequest: (url) => {
          const parsed = new URL(url, location.origin);
          if (parsed.origin !== location.origin && parsed.origin !== 'https://tiles.openfreemap.org' && parsed.origin !== SATELLITE_ORIGIN) throw new Error('This host is not an allowed public basemap origin.');
          return { url, credentials: 'same-origin' };
        },
      });
    } catch {
      setFatal('This browser could not start the globe. Evidence remains available in the report list.');
      return;
    }
    map.current = m;
    const preserveUserCamera = (event: { originalEvent?: unknown }) => { if (event.originalEvent) openingFit.current = false; };
    m.on('movestart', preserveUserCamera);
    if (new URLSearchParams(location.search).get('benchmark') === '1') window.__relicMap = m;
    m.addControl(new maplibregl.AttributionControl({ compact: true }), 'bottom-left');
    // On phones the expanded credits cover the status readout until the first
    // drag. Start collapsed there; the (i) button still opens them.
    if (host.current.offsetWidth <= 700) m.once('load', () => host.current?.querySelector('.maplibregl-ctrl-attrib')?.classList.remove('maplibregl-compact-show'));
    m.addControl(new maplibregl.ScaleControl({ maxWidth: 100, unit: 'metric' }), 'bottom-left');
    const hover = new maplibregl.Popup({ closeButton: false, closeOnClick: false, offset: 14, className: 'relic-globe-hover', maxWidth: '280px' });
    const publishCamera = () => {
      if (disposed) return;
      const center = m.getCenter(), rawBounds = m.getBounds();
      const bounds = normalizeViewportBounds([rawBounds.getWest(), rawBounds.getSouth(), rawBounds.getEast(), rawBounds.getNorth()]);
      if (!bounds) return;
      const snapshot: MapViewState = { center: [center.lng, center.lat], zoom: m.getZoom(), bearing: m.getBearing(), pitch: m.getPitch(), bounds, projection: projectionRef.current, basemap: basemapRef.current };
      try { localStorage.setItem(CAMERA_KEY, JSON.stringify(snapshot)); } catch { /* Camera remains usable for this session when storage is disabled. */ }
      latest.current.onMapState?.(snapshot);
      if (coordinateNode.current && !latestPointer) coordinateNode.current.textContent = coordinateLabel(center.lng, center.lat);
    };
    publishCameraRef.current = publishCamera;
    const applyVisibility = () => {
      for (const id of reportLayers) if (m.getLayer(id)) m.setLayoutProperty(id, 'visibility', latest.current.reportsVisible ? 'visible' : 'none');
    };
    const applySelection = () => {
      const source = m.getSource('selected-evidence') as maplibregl.GeoJSONSource | undefined;
      if (!source) return;
      const features = latest.current.result?.map.features.filter(f => f.properties.evidence_id === latest.current.selected.id && f.properties.revision === latest.current.selected.revision);
      source.setData({ type: 'FeatureCollection', features: features ?? [] } as FeatureCollection);
      const areas = latest.current.result?.map_areas.features.filter(f => latest.current.selectedAreas.includes(f.properties.area_id)) ?? [];
      (m.getSource('selected-areas') as maplibregl.GeoJSONSource | undefined)?.setData({type:'FeatureCollection',features:areas});
      applyVisibility();
    };
    applySelectionRef.current = applySelection;
    const applyReports = () => {
      clusterRequest++; hover.remove();
      // Refresh the bounded membership while keeping the chooser open through
      // live publications and rolling time ticks. Expired members fall out.
      if (menuState.current.choicesOpen) {
        if (chosenAreas.current.length) {
          const areas = latest.current.result?.map_areas.features.filter(a => chosenAreas.current.includes(a.properties.area_id)) ?? [];
          const members = areaChoices(areas);
          setChoiceTotal(areas.some(a => a.properties.message_count > a.properties.choices.length) || members.length > 100 ? -1 : members.length);
          setChoices(members.slice(0,100));
        } else {
          const fresh = latest.current.result?.map.features.map(f => mapChoice(f.properties)) ?? [];
          setChoices(current => current.flatMap(c => {
            const updated = fresh.find(f => f.id === c.id && f.revision === c.revision && f.site_key === c.site_key);
            return updated ? [updated] : [];
          }));
          setChoiceTotal(-1);
        }
      }
      (m.getSource('evidence') as maplibregl.GeoJSONSource | undefined)?.setData(reportFeatures(latest.current.result, eventMembers));
      const regions = regionLayers(latest.current.result?.map_areas);
      (m.getSource('evidence-areas') as maplibregl.GeoJSONSource | undefined)?.setData(regions.polygons);
      applySelection();
    };
    applyReportsRef.current = applyReports;
    const applyFly = () => {
      const intent = latest.current.flyTo;
      if (!intent || intent.nonce === lastFlyNonce) return;
      openingFit.current = false;
      if ('bounds' in intent) {
        lastFlyNonce = intent.nonce;
        const [w,s,e,n] = intent.bounds;
        m.fitBounds([[w,s],[e < w ? e + 360 : e,n]], {padding:visiblePadding(m),maxZoom:9,pitch:0,duration:cameraDuration(900)});
        return;
      }
      if (!Number.isFinite(intent.lng) || !Number.isFinite(intent.lat) || Math.abs(intent.lat) > 85) return;
      lastFlyNonce = intent.nonce;
      const pad = visiblePadding(m);
      m.flyTo({ center: [intent.lng, intent.lat], offset:[(pad.left-pad.right)/2,(pad.top-pad.bottom)/2], zoom: Math.min(19, Math.max(-0.75, intent.zoom ?? 8)), pitch: 0, duration: cameraDuration(1100), essential: false });
    };
    flyRef.current = applyFly;

    // Cluster text uses a canvas sprite independently of the basemap's glyphs.
    // At a 10k map membership cap, 192 distinct visible cluster sizes exceed the
    // possible simultaneous sizes. Evict old non-visible sizes across updates.
    m.on('styleimagemissing', event => {
      if (!event.id.startsWith('relic-count-') || m.hasImage(event.id)) return;
      const count = Number(event.id.slice('relic-count-'.length));
      if (!Number.isInteger(count) || count < 2 || count > 10000) return;
      if (countImages.size >= 192) {
        const counted = ['evidence-clusters', 'area-anchor'].filter(id => m.getLayer(id));
        const visible = new Set(m.queryRenderedFeatures({ layers: counted }).map(f => `relic-count-${f.properties?.reports ?? f.properties?.message_count}`));
        for (const old of countImages) { if (!visible.has(old)) { if (m.hasImage(old)) m.removeImage(old); countImages.delete(old); if (countImages.size < 160) break; } }
      }
      const canvas = document.createElement('canvas'); canvas.width = 88; canvas.height = 88;
      const ctx = canvas.getContext('2d'); if (!ctx) return;
      ctx.font = `700 ${count >= 1000 ? 21 : 24}px system-ui, sans-serif`; ctx.textAlign = 'center'; ctx.textBaseline = 'middle'; ctx.lineJoin = 'round';
      // White on a dark edge reads on every event colour.
      ctx.lineWidth = 5; ctx.strokeStyle = 'rgba(7,18,24,0.78)'; ctx.strokeText(count.toLocaleString('en-US'), 44, 44);
      ctx.fillStyle = '#ffffff'; ctx.fillText(count.toLocaleString('en-US'), 44, 44);
      m.addImage(event.id, ctx.getImageData(0, 0, 88, 88), { pixelRatio: 2 }); countImages.add(event.id);
    });
    const installReporting = () => {
      if (disposed) return;
      countImages.clear(); clusterRequest++; setChoices([]); hover.remove();
      const regions = regionLayers(latest.current.result?.map_areas);
      // Region outlines stay hidden until their marker is hovered; a selected
      // message's regions get a quiet dashed outline, never a large fill.
      if (!m.getSource('evidence-areas')) m.addSource('evidence-areas', {type:'geojson',promoteId:'area_id',attribution:'Facilities: <a href="https://www.openstreetmap.org/copyright">© OpenStreetMap contributors (ODbL)</a> · Boundaries: <a href="https://www.geoboundaries.org/">geoBoundaries</a> · Marine areas: <a href="https://www.marineregions.org/">Marine Regions / VLIZ (CC BY 4.0)</a>',data:regions.polygons});
      if (!m.getSource('selected-areas')) m.addSource('selected-areas', {type:'geojson',data:empty});
      const hovered: maplibregl.ExpressionSpecification = ['boolean', ['feature-state', 'hover'], false];
      if (!m.getLayer('area-fill')) m.addLayer({id:'area-fill',type:'fill',source:'evidence-areas',paint:{'fill-color':['get','color'],'fill-opacity':['case',hovered,0.08,0]}});
      if (!m.getLayer('area-outline')) m.addLayer({id:'area-outline',type:'line',source:'evidence-areas',paint:{'line-color':['get','color'],'line-width':1.4,'line-opacity':['case',hovered,0.85,0]}});
      if (!m.getLayer('selected-area-fill')) m.addLayer({id:'selected-area-fill',type:'fill',source:'selected-areas',paint:{'fill-color':'#dff3f5','fill-opacity':0.035}});
      if (!m.getLayer('selected-area-outline')) m.addLayer({id:'selected-area-outline',type:'line',source:'selected-areas',paint:{'line-color':'#dff3f5','line-width':1.3,'line-opacity':0.7,'line-dasharray':[2,1.6]}});
      if (!m.getSource('evidence')) m.addSource('evidence', { type: 'geojson', data: reportFeatures(latest.current.result, eventMembers), cluster: true, clusterMaxZoom: 17, clusterRadius: 44, clusterProperties, generateId: true });
      const single: maplibregl.FilterSpecification = ['all', ['!', ['has', 'point_count']], ['!', ['has', 'area_id']]];
      const region: maplibregl.FilterSpecification = ['all', ['!', ['has', 'point_count']], ['has', 'area_id']];
      const reports: maplibregl.ExpressionSpecification = ['get', 'reports'];
      // One ring per region: translucent, so it reads as "somewhere in here",
      // unlike the solid dot of a reported point.
      if (!m.getLayer('area-anchor-halo')) m.addLayer({id:'area-anchor-halo',type:'circle',source:'evidence',filter:region,paint:{'circle-color':['get','color'],'circle-opacity':0.15,'circle-radius':['step',['get','message_count'],13,10,16,100,20]}});
      if (!m.getLayer('area-anchor')) m.addLayer({id:'area-anchor',type:'circle',source:'evidence',filter:region,paint:{'circle-color':['get','color'],'circle-opacity':0.3,'circle-radius':['step',['get','message_count'],8,10,11,100,14],'circle-stroke-color':['get','color'],'circle-stroke-width':2,'circle-stroke-opacity':0.95}});
      if (!m.getLayer('area-anchor-count')) m.addLayer({id:'area-anchor-count',type:'symbol',source:'evidence',filter:['all',region,['>',['get','message_count'],1]],layout:{'icon-image':['concat','relic-count-',['to-string',['get','message_count']]],'icon-size':0.82,'icon-allow-overlap':true,'icon-ignore-placement':true,'icon-pitch-alignment':'viewport'}});
      if (!m.getLayer('evidence-halo')) m.addLayer({ id: 'evidence-halo', type: 'circle', source: 'evidence', filter: single, paint: { 'circle-radius': 12, 'circle-color': eventColor, 'circle-opacity': 0.16 } });
      if (!m.getLayer('evidence-cluster-halo')) m.addLayer({ id: 'evidence-cluster-halo', type: 'circle', source: 'evidence', filter: ['has', 'point_count'], paint: { 'circle-color': clusterColor, 'circle-opacity': 0.18, 'circle-radius': ['step', reports, 20, 10, 23, 100, 27] } });
      if (!m.getLayer('evidence-clusters')) m.addLayer({ id: 'evidence-clusters', type: 'circle', source: 'evidence', filter: ['has', 'point_count'], paint: { 'circle-color': clusterColor, 'circle-opacity': 0.92, 'circle-radius': ['step', reports, 13, 10, 16, 100, 19], 'circle-stroke-color': '#0b1a21', 'circle-stroke-width': 1.5 } });
      if (!m.getLayer('evidence-points')) m.addLayer({ id: 'evidence-points', type: 'circle', source: 'evidence', filter: single, paint: { 'circle-color': eventColor, 'circle-radius': 5, 'circle-stroke-color': '#0b1a21', 'circle-stroke-width': 1.5 } });
      if (!m.getLayer('evidence-counts')) m.addLayer({ id: 'evidence-counts', type: 'symbol', source: 'evidence', filter: ['has', 'point_count'], layout: { 'icon-image': ['concat', 'relic-count-', ['to-string', reports]], 'icon-allow-overlap': true, 'icon-ignore-placement': true, 'icon-pitch-alignment': 'viewport' } });
      if (!m.getSource('selected-evidence')) m.addSource('selected-evidence', { type: 'geojson', data: empty });
      if (!m.getLayer('selection-ring')) m.addLayer({ id: 'selection-ring', type: 'circle', source: 'selected-evidence', paint: { 'circle-radius': 24, 'circle-color': '#68dce1', 'circle-opacity': 0.09, 'circle-stroke-color': '#aff7f5', 'circle-stroke-width': 1.5, 'circle-stroke-opacity': 0.9 } });
      applySelection(); setReady(true); publishCamera(); applyFly();
    };
    m.on('style.load', installReporting);
    m.on('moveend', publishCamera);
    m.on('rotate', () => { if (compassNode.current) compassNode.current.style.transform = `rotate(${-m.getBearing()}deg)`; });
    m.on('error', event => {
      // Live layers (ships, aircraft, warnings) report their own failures in
      // the Layers panel; a failed ship tile is not missing basemap detail.
      if (disposed || String((event as { sourceId?: string }).sourceId ?? '').startsWith('live-')) return;
      setLoading(false);
      setNotice(detailedStyleLoaded.current ? 'Some map detail is unavailable. Loaded geography is retained.' : 'The map is unavailable. Check your connection and retry.');
    });
    m.on('idle', () => { if (!disposed && detailedStyleLoaded.current && !styleRequest) setLoading(false); });
    const loadMap = async () => {
      styleRequest?.abort(); clearTimeout(retryTimeout);
      const generation = ++styleGeneration;
      setNotice(''); hover.remove();
      // Retrying tiles leaves the current style and already loaded geography in
      // place. Never replace usable detail with the initial loading surface.
      if (detailedStyleLoaded.current) {
        setLoading(true);
        for (const [id, source] of Object.entries(m.getStyle().sources)) {
          if (source.type !== 'vector' && source.type !== 'raster' && source.type !== 'raster-dem') continue;
          const retainedSource = m.getSource(id) as maplibregl.VectorTileSource | maplibregl.RasterTileSource | undefined;
          // A failed TileJSON request has no tile templates to refresh. Retry
          // that source's metadata; keep sources with usable detail in place.
          if (retainedSource && !retainedSource.tiles?.length && source.url) retainedSource.setUrl(source.url);
          else m.refreshTiles(id);
        }
        m.triggerRepaint();
        return;
      }
      setLoading(true); styleRequest = new AbortController();
      retryTimeout = setTimeout(() => styleRequest?.abort(), 12000);
      try {
        const style = await onlineStyle(projectionRef.current, styleRequest.signal, basemapRef.current);
        if (disposed || generation !== styleGeneration) return;
        clearTimeout(retryTimeout); styleRequest = null; detailedStyleLoaded.current = true; style.projection = projectionSpec(projectionRef.current); setReady(false); m.setStyle(style, { diff: false });
      } catch {
        if (disposed || generation !== styleGeneration) return;
        clearTimeout(retryTimeout); styleRequest = null; setLoading(false);
        setNotice('The map is unavailable. Check your connection and retry.');
      }
    };
    retryMapRef.current = () => { void loadMap(); };
    // A new basemap is a whole new style; report and live layers reinstall on style.load.
    switchBasemapRef.current = mode => { if (mode === basemapRef.current) return; basemapRef.current = mode; setBasemap(mode); detailedStyleLoaded.current = false; void loadMap(); publishCamera(); };
    m.once('load', () => { void loadMap(); });

    // Region markers, not the whole region, take the pointer.
    const layersUnderPointer = () => ['evidence-points', 'evidence-clusters', 'area-anchor'].filter(id => m.getLayer(id));
    let hoveredArea: string | null = null;
    const hoverArea = (id: string | null) => {
      if (id === hoveredArea) return;
      if (hoveredArea && m.getSource('evidence-areas')) m.setFeatureState({ source: 'evidence-areas', id: hoveredArea }, { hover: false });
      hoveredArea = id;
      if (id && m.getSource('evidence-areas')) m.setFeatureState({ source: 'evidence-areas', id }, { hover: true });
    };
    m.on('mousemove', event => {
      latestPointer = event;
      if (cursorFrame) return;
      cursorFrame = requestAnimationFrame(() => {
        cursorFrame = 0; const pointer = latestPointer; if (!pointer || disposed) return;
        const projected = m.project(pointer.lngLat);
        const onSurface = Number.isFinite(pointer.lngLat.lng) && Number.isFinite(pointer.lngLat.lat) && Math.hypot(projected.x - pointer.point.x, projected.y - pointer.point.y) < 3;
        if (coordinateNode.current) coordinateNode.current.textContent = onSurface ? coordinateLabel(pointer.lngLat.lng, pointer.lngLat.lat) : 'Outside globe';
        if (!onSurface) { hover.remove(); hoverArea(null); m.getCanvas().style.cursor = ''; return; }
        const layers = layersUnderPointer();
        const feature = layers.length && latest.current.reportsVisible ? m.queryRenderedFeatures(pointer.point, { layers })[0] : undefined;
        // A ship or aircraft near the pointer outranks a background area; the
        // live layers show their own preview.
        if ((!feature || feature.properties.area_id) && liveFeatureNear(m, pointer.point)) { hover.remove(); hoverArea(null); m.getCanvas().style.cursor = 'pointer'; return; }
        hoverArea(feature?.properties.area_id ? String(feature.properties.area_id) : null);
        m.getCanvas().style.cursor = feature ? 'pointer' : '';
        if (!feature) { hover.remove(); return; }
        const properties = feature.properties;
        const element = document.createElement('div');
        const caption = document.createElement('small'); caption.textContent = properties.area_id ? `Somewhere in this ${areaKind(String(properties.level))}` : properties.point_count ? 'Reports here' : latest.current.sourceLabels[properties.source] || 'Message';
        const preview = cleanReadingText(String(properties.title || ''));
        const text = document.createElement('strong'); text.textContent = properties.area_id ? `${placeLabel({ id: String(properties.area_id), label: String(properties.label), level: String(properties.level), country_code: properties.country_code ? String(properties.country_code) : null })} · ${Number(properties.message_count).toLocaleString()} ${Number(properties.message_count) === 1 ? 'message' : 'messages'}` : properties.point_count ? `${Number(properties.reports ?? properties.point_count).toLocaleString()} reports · click to zoom in` : preview.length > 240 ? `${preview.slice(0, 240)}…` : preview;
        element.append(caption, text); hover.setLngLat(pointer.lngLat).setDOMContent(element).addTo(m);
      });
    });
    m.getCanvas().addEventListener('mouseleave', () => { latestPointer = null; hover.remove(); hoverArea(null); m.getCanvas().style.cursor = ''; if (coordinateNode.current) { const center = m.getCenter(); coordinateNode.current.textContent = coordinateLabel(center.lng, center.lat); } });
    m.on('movestart', () => hover.remove());
    m.on('click', async event => {
      if (!latest.current.reportsVisible) return;
      const layers = layersUnderPointer(); if (!layers.length) return;
      const hits = m.queryRenderedFeatures(event.point, { layers });
      // A ship or aircraft under the pointer wins over a background area;
      // a report point or cluster always wins over both.
      if (!hits.some(h => !h.properties.area_id) && liveFeatureNear(m, event.point)) { setChoices([]); return; }
      const feature = hits[0]; if (!feature) { setChoices([]); return; }
      hover.remove();
      const properties = feature.properties;
      if (properties.area_id) {
        const areas = latest.current.result?.map_areas.features.filter(a => hits.some(h => h.properties.area_id === a.properties.area_id)) ?? [];
        setChoicePlace(areas.length === 1 ? `${placeLabel({id:areas[0].properties.area_id,...areas[0].properties})} · ${areaKind(areas[0].properties.level)} area` : "Messages in overlapping areas");
        chosenAreas.current = areas.map(a => a.properties.area_id);
        const members = areaChoices(areas);
        if (members.length === 1) latest.current.onSelect(members[0].id, members[0].revision);
        else { setChoiceTotal(areas.some(a => a.properties.message_count > a.properties.choices.length) || members.length > 100 ? -1 : members.length); setChoices(members.slice(0,100)); }
        return;
      }
      chosenAreas.current = [];
      setChoicePlace('Messages at this location');
      if (!properties.point_count) {
        const points = hits.filter(f => !f.properties.point_count && !f.properties.area_id);
        const expanded: Record<string, unknown>[] = points.flatMap(f => eventMembers.get(String(f.properties.event_key))?.map(x => x.properties) ?? [f.properties]);
        const unique = [...new Map(expanded.map(p => [`${p.evidence_id}@${p.revision}`, p])).values()];
        if (points.length === 1 && Number(properties.site_reports) > 1) setChoicePlace('Reports of the same event at this location');
        if (unique.length > 1) { setChoiceTotal(unique.length); setChoices(unique.slice(0, 100).map(mapChoice)); }
        else latest.current.onSelect(String(properties.evidence_id), String(properties.revision));
        return;
      }
      const request = ++clusterRequest, source = m.getSource('evidence') as maplibregl.GeoJSONSource;
      try {
        const [expansionZoom, leaves] = await Promise.all([source.getClusterExpansionZoom(Number(properties.cluster_id)), source.getClusterLeaves(Number(properties.cluster_id), 100, 0)]);
        if (disposed || request !== clusterRequest) return;
        const first = leaves[0]?.geometry as Point | undefined;
        const colocated = Number(properties.point_count) <= 100 && !!first && leaves.every(f => f.geometry.type === 'Point' && Math.abs(f.geometry.coordinates[0] - first.coordinates[0]) < 0.000001 && Math.abs(f.geometry.coordinates[1] - first.coordinates[1]) < 0.000001);
        if (colocated || m.getZoom() >= 17.75 || expansionZoom > 18) {
          // Region markers in the cluster contribute their own messages.
          const regionIds = new Set(leaves.flatMap(f => f.properties?.area_id ? [String(f.properties.area_id)] : []));
          const regions = latest.current.result?.map_areas.features.filter(a => regionIds.has(a.properties.area_id)) ?? [];
          const expanded: Record<string, unknown>[] = leaves.filter(f => !f.properties?.area_id).flatMap(f => eventMembers.get(String(f.properties?.event_key))?.map(x => x.properties) ?? [f.properties ?? {}]);
          const unique = [...new Map([...expanded.map(p => mapChoice(p)), ...areaChoices(regions)].map(c => [`${c.id}@${c.revision}`, c])).values()];
          const partial = Number(properties.point_count) > leaves.length || regions.some(a => a.properties.message_count > a.properties.choices.length);
          setChoiceTotal(partial ? -1 : unique.length); setChoices(unique.slice(0, 100));
        } else { setChoices([]); m.easeTo({ center: (feature.geometry as Point).coordinates as [number, number], zoom: Math.min(18, Math.max(m.getZoom() + 0.7, expansionZoom)), duration: cameraDuration(650) }); }
      } catch { if (!disposed && request === clusterRequest) setNotice('This cluster changed while opening. Select it again to inspect the current report membership.'); }
    });
    const resize = new ResizeObserver(() => m.resize()); resize.observe(host.current);
    return () => {
      disposed = true; map.current = null; styleRequest?.abort(); clearTimeout(retryTimeout); cancelAnimationFrame(cursorFrame); hover.remove(); resize.disconnect();
      if (window.__relicMap === m) delete window.__relicMap;
      map.current = null; m.remove();
    };
  }, []);

  useEffect(() => {
    const dismissOnPointer = (event: PointerEvent) => {
      const target = event.target instanceof Element ? event.target : null;
      if (menuState.current.styleOpen && !target?.closest('.globe-style-menu, .globe-style-toggle')) setStyleMenu(false);
      if (menuState.current.choicesOpen && !target?.closest('.globe-location-chooser')) setChoices([]);
    };
    const dismissOnEscape = (event: KeyboardEvent) => {
      if (event.key !== 'Escape' || (!menuState.current.styleOpen && !menuState.current.choicesOpen)) return;
      event.preventDefault(); event.stopPropagation();
      const wasStyle = menuState.current.styleOpen;
      setStyleMenu(false); setChoices([]);
      if (wasStyle) host.current?.parentElement?.querySelector<HTMLButtonElement>('.globe-style-toggle')?.focus({ preventScroll: true });
      else map.current?.getCanvas().focus({ preventScroll: true });
    };
    document.addEventListener('pointerdown', dismissOnPointer, true);
    window.addEventListener('keydown', dismissOnEscape, true);
    return () => { document.removeEventListener('pointerdown', dismissOnPointer, true); window.removeEventListener('keydown', dismissOnEscape, true); };
  }, []);
  useEffect(() => { if (ready) applyReportsRef.current(); }, [ready, result?.map, result?.map_areas]);
  useEffect(() => { if (ready) applySelectionRef.current(); if (!reportsVisible) setChoices([]); }, [ready, selected.id, selected.revision, reportsVisible, areaSelection.data]);
  useEffect(() => { if (ready) flyRef.current(); }, [ready, flyTo?.nonce]);
  useEffect(() => {
    if (ready && openingFit.current && (result?.map.features.length || result?.map_areas.features.length)) {
      openingFit.current = false;
      fit();
    }
  }, [ready, result?.map, result?.map_areas]);

  const toggleProjection = () => {
    const m = map.current; if (!m || !ready) return;
    openingFit.current = false;
    const next: ProjectionMode = projectionRef.current === 'globe' ? 'mercator' : 'globe';
    // Stop an in-flight camera animation at its current position, then
    // switch once. A zoom-driven morph can survive MapLibre's `globe` alias
    // short-circuit and re-trigger whenever that zoom is crossed later.
    m.stop();
    m.setProjection(projectionSpec(next));
    projectionRef.current = next; setProjection(next); publishCameraRef.current();
  };
  const reset = () => {
    const m = map.current; if (!m) return;
    openingFit.current = false;
    const canvas = m.getContainer();
    m.flyTo({ ...homeCamera(canvas.clientWidth, canvas.clientHeight), duration: cameraDuration(1200) });
  };
  const fit = () => {
    const features = result?.map.features; if (!map.current || (!features?.length && !result?.map_areas.features.length)) return;
    const bounds = new maplibregl.LngLatBounds(); features?.forEach(f => bounds.extend(f.geometry.coordinates as [number, number]));
    result?.map_areas.features.forEach(f => { const [w,s,e,n] = f.properties.bounds; bounds.extend([w,s]); bounds.extend([e,n]); });
    openingFit.current = false;
    map.current.fitBounds(bounds, { padding: visiblePadding(map.current),  maxZoom: 8, duration: cameraDuration(850) });
  };
  return <section className="map-pane relic-globe-map" aria-label="Interactive geographic globe" data-projection={projection} data-basemap={basemap}>
    <div className="map-canvas" ref={host} data-testid="map-canvas" />
    {fatal && <div className="globe-status globe-fatal" role="alert">{fatal}</div>}
    {!fatal && notice && <div className="globe-status" role="status"><WifiOff size={14}/><span>{notice}</span><button onClick={() => retryMapRef.current()}>Retry</button><button className="globe-dismiss" aria-label="Dismiss basemap status" onClick={() => setNotice('')}><X size={13}/></button></div>}
    {choices.length > 0 && <div className="globe-location-chooser" role="region" aria-label="Messages at selected location"><header><div><small>{choicePlace}</small><strong>{choiceTotal < 0 ? `Showing ${choices.length} messages` : `${choiceTotal.toLocaleString()} messages`} · newest first</strong></div><button aria-label="Close reports at location" onClick={() => setChoices([])}><X size={16}/></button></header><div className="globe-location-items">{[...choices].sort((a,b) => (b.published_at || b.acquired_at || "").localeCompare(a.published_at || a.acquired_at || "")).map(choice => <button className="globe-location-row" key={`${choice.id}@${choice.revision}`} onClick={() => { onSelect(choice.id, choice.revision); setChoices([]); }}><small className="globe-choice-source">{sourceLabels[choice.source] || 'Message'}</small><MessageTime {...choice}/><EventLabel reading={{events:choice.events}}/>{choice.location && <small className="globe-choice-place">{choice.location}</small>}{choice.processed === false && <small>Original · no reading yet</small>}<span className="globe-choice-text" dir="auto">{cleanReadingText(choice.title) || 'Media-only message'}</span></button>)}</div>{(choiceTotal < 0 || choiceTotal > choices.length) && <p>Showing {choices.length} messages. Use the feed for the full selection.</p>}</div>}
    {reportsVisible && ready && <div className="event-legend" aria-label="Map colours">{eventGroups.map(group => <span key={group}><i style={{ background: groupColors[group] }}/>{legendNames[group] ?? group}</span>)}<span title="A report placed only somewhere in a country, province or sea"><i className="ring"/>Region</span></div>}
    <MapPreview map={map} ready={ready && reportsVisible} at={preview?.at ?? null} content={preview?.content ?? null} onOpen={() => onOpen?.()} onClose={() => onClosePreview?.()}/>
    <div className="globe-map-tools">
      <LiveLayers map={map} ready={ready} historical={historical} historicalAt={historicalAt} onSelect={onSelect}/>
      {styleMenu && <div className="globe-style-menu"><div className="globe-basemap" role="group" aria-label="Basemap">{(['online', 'satellite'] as const).map(mode => <button key={mode} aria-pressed={basemap === mode} onClick={() => { switchBasemapRef.current(mode); setStyleMenu(false); }}>{mode === 'online' ? 'Dark map' : 'Satellite'}</button>)}</div><div className="globe-menu-actions"><button onClick={() => { map.current?.easeTo({ bearing: 0, pitch: 0, duration: cameraDuration(500) }); setStyleMenu(false); }}><Compass size={16} ref={compassNode} style={{ transform: `rotate(${-((map.current?.getBearing()) ?? 0)}deg)` }}/><span>Reset north and pitch</span></button><button disabled={!result?.map_loaded && !result?.map_areas.features.length} onClick={() => { fit(); setStyleMenu(false); }}><LocateFixed size={16}/><span>Fit reports</span></button><button onClick={() => { reset(); setStyleMenu(false); }}><RotateCcw size={16}/><span>View the world</span></button></div></div>}
      <button className="globe-style-toggle" aria-label="Map navigation" aria-expanded={styleMenu} onClick={() => setStyleMenu(!styleMenu)}><Compass size={14}/><span>Map</span>{loading ? <i role="status" aria-label="Loading map" className="globe-loading-dot"/> : <ChevronDown size={11}/>}</button>
      <div className="globe-tool-group"><button aria-label={projection === 'globe' ? 'Switch to flat map' : 'Switch to globe'} title={projection === 'globe' ? 'Switch to flat map' : 'Switch to globe'} onClick={toggleProjection} disabled={!ready}>{projection === 'globe' ? <Globe2 size={18}/> : <FlatMap size={18}/>}<span>{projection === 'globe' ? '3D' : '2D'}</span></button></div>
      <div className="globe-tool-group"><button aria-label="Zoom in" title="Zoom in" onClick={() => map.current?.zoomIn({ duration: cameraDuration(250) })}><Plus size={18}/></button><button aria-label="Zoom out" title="Zoom out" onClick={() => map.current?.zoomOut({ duration: cameraDuration(250) })}><Minus size={18}/></button></div>
    </div>
    <div className="globe-map-readout"><span ref={coordinateNode}>{coordinateLabel(initial.current.center[0], initial.current.center[1])}</span>{fictional && reportsVisible && (!!result?.map_loaded || !!result?.map_areas.features.length) && <span>Fictional reports</span>}{updating && <span className="globe-updating">Updating reports</span>}</div>
  </section>;
}
