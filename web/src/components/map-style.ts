import type { StyleSpecification } from 'maplibre-gl';

export type BasemapMode = 'online' | 'satellite';
export type ProjectionMode = 'globe' | 'mercator';
// Use the fixed projections behind the two user modes. MapLibre's `globe`
// alias is itself zoom-dependent; 3D must remain the analyst's explicit choice.
export function projectionSpec(mode: ProjectionMode): NonNullable<StyleSpecification['projection']> {
  return { type: mode === 'globe' ? 'vertical-perspective' : 'mercator' };
}
export const ONLINE_STYLE_URL = 'https://tiles.openfreemap.org/styles/dark';
// Satellite imagery under OpenFreeMap's borders, roads and labels (owner
// request, 26 October 2026; Recall used the same imagery). Raster tiles only.
export const SATELLITE_ORIGIN = 'https://server.arcgisonline.com';
const SATELLITE_TILES = `${SATELLITE_ORIGIN}/ArcGIS/rest/services/World_Imagery/MapServer/tile/{z}/{y}/{x}`;
export const HOME_CAMERA = { center: [-12, 28] as [number, number], zoom: 2.1, bearing: 0, pitch: 0 };
export const CAMERA_KEY = 'relic:globe-camera:v1';
export type SavedCamera = typeof HOME_CAMERA & { projection: ProjectionMode; basemap: BasemapMode };

export function homeCamera(width: number, height: number) {
  // Keep the opening globe proportional to the available canvas, without
  // changing a saved camera or refitting when evidence/drawers update.
  const shortSide = Math.max(320, Math.min(width, height));
  return { ...HOME_CAMERA, zoom: Math.min(4, HOME_CAMERA.zoom + Math.log2(shortSide / 720)) };
}

export function readCamera(): SavedCamera & { restored: boolean } {
  const defaults = { ...homeCamera(window.innerWidth, window.innerHeight), projection: 'globe' as const, basemap: 'online' as BasemapMode, restored: false };
  try {
    const value = JSON.parse(localStorage.getItem(CAMERA_KEY) || 'null') as Partial<SavedCamera> | null;
    if (!value || !Array.isArray(value.center) || value.center.length !== 2 || !value.center.every(Number.isFinite) || Math.abs(value.center[1]) > 85 || !Number.isFinite(value.zoom) || !Number.isFinite(value.bearing) || !Number.isFinite(value.pitch)) return defaults;
    // Older sessions could select a local locator. Keep their camera, while
    // moving every session to a supported online basemap.
    return { center: [((value.center[0] + 180) % 360 + 360) % 360 - 180, value.center[1]], zoom: Math.min(19, Math.max(-0.75, value.zoom!)), bearing: value.bearing!, pitch: Math.min(65, Math.max(0, value.pitch!)), projection: value.projection === 'mercator' ? 'mercator' : 'globe', basemap: value.basemap === 'satellite' ? 'satellite' : 'online', restored: true };
  } catch { return defaults; }
}
export function sky() {
  return { 'sky-color': '#060b11', 'horizon-color': '#173244', 'fog-color': '#101e2a', 'sky-horizon-blend': 0.7, 'horizon-fog-blend': 0.65, 'atmosphere-blend': ['interpolate', ['linear'], ['zoom'], 0, 0.75, 3, 0.25, 6, 0] } satisfies NonNullable<StyleSpecification['sky']>;
}
// A source-free loading surface, not an alternate map. Geographic detail comes
// only from the online style below; failure is reported explicitly by MapPane.
export function loadingStyle(projection: ProjectionMode): StyleSpecification {
  return {
    version: 8, name: 'Relic map loading', projection: projectionSpec(projection), sky: sky(),
    sources: {},
    layers: [{ id: 'loading-background', type: 'background', paint: { 'background-color': '#0e1c28' } }],
  };
}

// Public cartographic display only. This never routes evidence/provider requests.
// Verified provider contract: https://openfreemap.org/quick_start/
// All style, glyph, sprite and tile URLs are restricted to this one public host.
export async function onlineStyle(projection: ProjectionMode, signal: AbortSignal, basemap: BasemapMode = 'online'): Promise<StyleSpecification> {
  const response = await fetch(ONLINE_STYLE_URL, { signal, credentials: 'omit', redirect: 'error', referrerPolicy: 'no-referrer' });
  if (!response.ok) throw new Error('The detailed basemap style is unavailable.');
  const text = await response.text();
  if (text.length > 2 * 1024 * 1024) throw new Error('The detailed basemap style exceeded its size limit.');
  const style = JSON.parse(text) as StyleSpecification;
  if (style.version !== 8 || !Array.isArray(style.layers) || style.layers.length > 500 || !style.sources) throw new Error('The detailed basemap style could not be validated.');
  const allowed = (url: string) => { if (new URL(url).origin !== 'https://tiles.openfreemap.org') throw new Error('The public basemap requested an unapproved origin.'); };
  if (style.glyphs) allowed(style.glyphs);
  if (typeof style.sprite === 'string') allowed(style.sprite);
  else style.sprite?.forEach(sprite => allowed(sprite.url));
  for (const source of Object.values(style.sources)) {
    if ('url' in source && typeof source.url === 'string') allowed(source.url);
    if ('tiles' in source && Array.isArray(source.tiles)) source.tiles.forEach(allowed);
  }
  style.name = 'Relic · OpenFreeMap Dark';
  style.projection = projectionSpec(projection);
  style.sky = sky();
  style.transition = { duration: 250, delay: 0 };
  for (const layer of style.layers) {
    if (layer.type === 'background') layer.paint = { ...layer.paint, 'background-color': '#253440' };
    if (layer.type === 'fill') {
      if (layer['source-layer'] === 'water') layer.paint = { ...layer.paint, 'fill-color': '#0e1c28' };
      else if (layer['source-layer'] === 'landcover') layer.paint = { ...layer.paint, 'fill-color': '#263a43' };
      else if (layer['source-layer'] === 'landuse') layer.paint = { ...layer.paint, 'fill-color': '#26343e' };
      else if (layer['source-layer'] === 'building') layer.paint = { ...layer.paint, 'fill-color': '#334551' };
    }
    if (layer.type === 'line' && layer['source-layer'] === 'boundary') layer.paint = { ...layer.paint, 'line-color': '#788a99', 'line-opacity': 0.38 };
    if (layer.type === 'symbol') {
      layer.paint = { ...layer.paint, 'text-color': '#a8b8c4', 'text-halo-color': '#14232e', 'text-halo-width': 1.2, 'text-halo-blur': 0.3 };
      if (JSON.stringify(layer.layout?.['text-field'] ?? '').includes('name')) layer.layout = { ...layer.layout, 'text-field': ['coalesce', ['get', 'name_en'], ['get', 'name:latin'], ['get', 'name']], 'text-padding': 5 };
      if (layer.id.startsWith('place_country')) {
        layer.layout = { ...layer.layout, 'text-padding': 12, 'text-max-width': 16, 'text-letter-spacing': 0.07 };
        if (layer.id === 'place_country_minor') layer.minzoom = Math.max(layer.minzoom ?? 0, 2.5);
        if (layer.id === 'place_country_other') layer.minzoom = Math.max(layer.minzoom ?? 0, 3.5);
      }
      if (layer.id === 'place_city') layer.minzoom = Math.max(layer.minzoom ?? 0, 5);
      if (layer.id === 'place_city_large') layer.minzoom = Math.max(layer.minzoom ?? 0, 4);
      if (layer.id === 'place_state') layer.minzoom = Math.max(layer.minzoom ?? 0, 4.5);
    }
  }
  return basemap === 'satellite' ? satellite(style) : style;
}

/** The dark style over satellite imagery: flat fills hidden, borders and labels
 *  brightened for imagery, the imagery dimmed slightly so reports stand out. */
function satellite(style: StyleSpecification): StyleSpecification {
  style.name = 'Relic · Satellite (Esri World Imagery) with OpenFreeMap labels';
  style.sources.satellite = { type: 'raster', tiles: [SATELLITE_TILES], tileSize: 256, maxzoom: 19, attribution: '© Esri, Maxar, Earthstar Geographics' };
  const layers = style.layers.filter(layer => layer.type !== 'fill' && layer.type !== 'fill-extrusion');
  for (const layer of layers) {
    if (layer.type === 'line' && layer['source-layer'] === 'boundary') layer.paint = { ...layer.paint, 'line-color': '#e6eef3', 'line-opacity': 0.5 };
    else if (layer.type === 'line' && layer['source-layer'] === 'transportation') layer.paint = { ...layer.paint, 'line-opacity': 0.35 };
    else if (layer.type === 'line' && layer['source-layer'] === 'waterway') layer.layout = { ...layer.layout, visibility: 'none' };
    if (layer.type === 'symbol') layer.paint = { ...layer.paint, 'text-color': '#f1f5f8', 'text-halo-color': '#0a1218', 'text-halo-width': 1.4, 'text-halo-blur': 0.4 };
  }
  const background = layers.findIndex(layer => layer.type === 'background');
  layers.splice(background + 1, 0, { id: 'satellite', type: 'raster', source: 'satellite', paint: { 'raster-brightness-max': 0.86, 'raster-saturation': -0.08, 'raster-fade-duration': 150 } });
  style.layers = layers;
  return style;
}

export function coordinateLabel(lng: number, lat: number) {
  const wrapped = ((lng + 180) % 360 + 360) % 360 - 180;
  return `${Math.abs(lat).toFixed(3)}°${lat >= 0 ? 'N' : 'S'}  ${Math.abs(wrapped).toFixed(3)}°${wrapped >= 0 ? 'E' : 'W'}`;
}

// The query contract permits wrapped rectangles (west > east), but every
// coordinate must be finite/in range. A full world width retains both endpoints.
export function normalizeViewportBounds(bounds: [number, number, number, number]): [number, number, number, number] | null {
  const [west, south, east, north] = bounds;
  if (!bounds.every(Number.isFinite) || south > north) return null;
  const latitude = (value: number) => Math.min(90, Math.max(-90, value));
  if (Math.abs(east - west) >= 360 - 1e-7) return [-180, latitude(south), 180, latitude(north)];
  const longitude = (value: number) => { const wrapped = ((value + 180) % 360 + 360) % 360 - 180; return wrapped === -180 && value > 0 ? 180 : wrapped; };
  return [longitude(west), latitude(south), longitude(east), latitude(north)];
}
