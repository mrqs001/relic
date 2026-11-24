// Offline regression for the explicit 2D/3D projection contract. Uses the
// installed MapLibre expression evaluator, the source-free loading surface,
// and the online basemap. No geography is substituted when the provider fails.
// Run after `npm ci --prefix web`: node validation/check_map_projection.mjs
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { createRequire } from 'node:module';

const require = createRequire(new URL('../web/package.json', import.meta.url));
const { transform } = require('esbuild');
const { createPropertyExpression, latest, validateStyleMin } = require('@maplibre/maplibre-gl-style-spec');
const { code } = await transform(readFileSync(new URL('../web/src/components/map-style.ts', import.meta.url), 'utf8'), { loader: 'ts', format: 'esm' });
const { loadingStyle, onlineStyle, projectionSpec, readCamera, CAMERA_KEY, ONLINE_STYLE_URL, SATELLITE_ORIGIN } = await import(`data:text/javascript;base64,${Buffer.from(code).toString('base64')}`);

// The network-free provider fixture also verifies that a style's own projection
// cannot override the user's selected mode when a basemap finishes loading.
const originalFetch = globalThis.fetch;
const requestedUrls = [];
globalThis.fetch = async (url) => {
  requestedUrls.push(url);
  return new Response(JSON.stringify({
  version: 8,
  projection: { type: 'globe' },
  sources: { openmaptiles: { type: 'vector', url: 'https://tiles.openfreemap.org/planet' } },
  layers: [{ id: 'background', type: 'background', paint: { 'background-color': '#000000' } }, { id: 'water', type: 'fill', source: 'openmaptiles', 'source-layer': 'water', paint: {} }, { id: 'boundary', type: 'line', source: 'openmaptiles', 'source-layer': 'boundary', paint: {} }],
  }), { status: 200 });
};

let evaluations = 0;
try {
  for (const [mode, expected] of [['globe', 'vertical-perspective'], ['mercator', 'mercator']]) {
    assert.deepEqual(projectionSpec(mode), { type: expected });
    const loading = loadingStyle(mode);
    assert.deepEqual(loading.sources, {}, 'The loading surface must not request a local or offline map.');
    assert.deepEqual(loading.layers.map(layer => layer.type), ['background']);
    assert.equal(loading.glyphs, undefined);
    assert.equal(loading.sprite, undefined);
    const styles = [loading, await onlineStyle(mode, new AbortController().signal)];
    for (const style of styles) {
      assert.deepEqual(validateStyleMin(style).map(error => error.message), []);
      assert.deepEqual(style.projection, { type: expected });
      const expression = createPropertyExpression(style.projection.type, latest.projection.type);
      assert.equal(expression.result, 'success');
      assert.equal(expression.value.kind, 'constant', 'An explicit mode must not depend on camera zoom.');
      // Include the old 0.02-wide failure interval and MapLibre's globe-alias
      // flattening range (11–12), as well as the entire supported camera range.
      for (const zoom of [-0.75, 0, 2.1, 2.11, 2.12, 3, 6, 9.5, 10, 11, 11.5, 12, 16, 19]) {
        assert.equal(expression.value.evaluate({ zoom }), expected, `${mode} changed projection at zoom ${zoom}`);
        evaluations++;
      }
    }
  }
  // Satellite: Esri imagery directly above the background, flat fills hidden,
  // OpenFreeMap borders and labels kept; tiles only from the imagery origin.
  const satellite = await onlineStyle('mercator', new AbortController().signal, 'satellite');
  assert.deepEqual(validateStyleMin(satellite).map(error => error.message), []);
  assert.deepEqual(satellite.layers.map(layer => layer.id), ['background', 'satellite', 'boundary']);
  assert.ok(satellite.sources.satellite.tiles.every(url => new URL(url).origin === SATELLITE_ORIGIN));
  assert.match(satellite.sources.satellite.attribution, /Esri/);
  assert.deepEqual(requestedUrls, [ONLINE_STYLE_URL, ONLINE_STYLE_URL, ONLINE_STYLE_URL], 'The only map style request must use the approved online origin.');
  globalThis.fetch = async () => { throw new Error('Network unavailable'); };
  await assert.rejects(onlineStyle('globe', new AbortController().signal), /Network unavailable/, 'A failed online style must never resolve to a local fallback.');
} finally {
  globalThis.fetch = originalFetch;
}

const originalWindow = globalThis.window, originalStorage = Object.getOwnPropertyDescriptor(globalThis, 'localStorage');
try {
  const previousCamera = { center: [-9.147, 38.725], zoom: 9.5, bearing: 17, pitch: 21, projection: 'mercator', basemap: 'local' };
  globalThis.window = { innerWidth: 1280, innerHeight: 720 };
  Object.defineProperty(globalThis, 'localStorage', { configurable: true, writable: true, value: { getItem(key) { assert.equal(key, CAMERA_KEY); return JSON.stringify(previousCamera); } } });
  const migratedCamera = readCamera();
  assert.equal(migratedCamera.basemap, 'online');
  previousCamera.basemap = 'satellite';
  assert.equal(readCamera().basemap, 'satellite', 'A saved satellite choice is kept.');
  previousCamera.basemap = 'local';
  assert.equal(migratedCamera.projection, previousCamera.projection);
  assert.equal(migratedCamera.zoom, previousCamera.zoom);
  assert.equal(migratedCamera.bearing, previousCamera.bearing);
  assert.equal(migratedCamera.pitch, previousCamera.pitch);
  assert.ok(Math.abs(migratedCamera.center[0] - previousCamera.center[0]) < 1e-10);
  assert.equal(migratedCamera.center[1], previousCamera.center[1]);
} finally {
  if (originalWindow === undefined) delete globalThis.window; else globalThis.window = originalWindow;
  if (originalStorage === undefined) delete globalThis.localStorage; else Object.defineProperty(globalThis, 'localStorage', originalStorage);
}

console.log(JSON.stringify({ outcome: 'passed', styles: 5, projection_evaluations: evaluations, zoom_range: [-0.75, 19], online_only: true, satellite: 'passed', old_camera_migration: 'passed', network_requests: 0 }));
