import { useEffect, useRef, useState } from 'react';
import type { Map as MapLibreMap, GeoJSONSource } from 'maplibre-gl';

type Result = {
  mode: string; elapsed_ms: number; frames: number; frame_p95_ms: number; frame_max_ms: number;
  stalls_over_100_ms: number; submitted_changes: number; contacts: number; inherited_geometry_count: number;
  heap_start_bytes: number | null; heap_end_bytes: number | null; map_idle_at_stop: boolean;
  browser: string; viewport: { width: number; height: number; pixel_ratio: number }; request_origins: string[];
  webgl_renderer: string; error?: string;
};
const memory = () => (performance as Performance & { memory?: { usedJSHeapSize: number } }).memory?.usedJSHeapSize ?? null;
const q = (numbers: number[], p: number) => numbers.length ? [...numbers].sort((a, b) => a - b)[Math.min(numbers.length - 1, Math.floor(numbers.length * p))] : 0;

// Deliberately opt-in rendering instrument. This never writes evidence or calls a provider.
export default function Benchmark() {
  const [result, setResult] = useState<Result | null>(null);
  const [interactions, setInteractions] = useState<{ kind: string; duration_ms: number }[]>([]);
  const [status, setStatus] = useState('Ready — fictional rendering workload only');
  const stop = useRef<(() => void) | null>(null);
  useEffect(() => () => stop.current?.(), []);
  useEffect(() => {
    let selectedAt = 0, scopeAt = 0, saveAt = 0;
    let alive = true;
    const record = (kind: string, began: number) => {
      requestAnimationFrame(() => { if (alive) setInteractions(old => [...old.slice(-49), { kind, duration_ms: Number((performance.now() - began).toFixed(2)) }]); });
    };
    const click = (event: Event) => {
      const element = event.target instanceof Element ? event.target : null;
      if (element?.closest('button.evidence-row')) selectedAt = performance.now();
      const button = element?.closest('button');
      if (button && /^Save (notes|assessment|brief|new brief revision|pin)/i.test(button.textContent?.trim() || '')) {
        const started = performance.now(); saveAt = started;
        const feedback = () => {
          if (!alive) return;
          if (button.disabled || /saving|saved/i.test(button.textContent || '')) record('save_feedback', started);
          else if (performance.now() - started < 2000 && saveAt === started) requestAnimationFrame(feedback);
        };
        requestAnimationFrame(feedback);
      }
    };
    const scopeChange = (event: Event) => { if (event.target instanceof Element && event.target.closest('.scope-bar')) scopeAt = performance.now(); };
    const observer = new PerformanceObserver(list => {
      for (const entry of list.getEntries()) {
        if (entry.name === 'relic:evidence-visible' && selectedAt) { record('evidence_visible', selectedAt); selectedAt = 0; }
        if (entry.name === 'relic:query-visible' && scopeAt) { record('scope_visible', scopeAt); scopeAt = 0; }
        if (entry.name === 'relic:save-ack' && saveAt) { record('save_durable_visible', saveAt); saveAt = 0; }
      }
    });
    observer.observe({ type: 'mark' });
    document.addEventListener('click', click, true);
    document.addEventListener('input', scopeChange, true);
    document.addEventListener('change', scopeChange, true);
    return () => { alive = false; observer.disconnect(); document.removeEventListener('click', click, true); document.removeEventListener('input', scopeChange, true); document.removeEventListener('change', scopeChange, true); };
  }, []);

  async function start(mode: 'normal' | 'overload') {
    stop.current?.();
    const map = (window as Window & { __relicMap?: MapLibreMap }).__relicMap;
    if (!map || !map.isStyleLoaded()) { setStatus('Map is not ready. Wait for the globe to finish loading.'); return; }
    const original = { center: map.getCenter(), zoom: map.getZoom(), bearing: map.getBearing(), pitch: map.getPitch() };
    const sourceId = 'relic-benchmark-contacts';
    const features: GeoJSON.Feature<GeoJSON.Point>[] = Array.from({ length: 10000 }, (_, i) => ({
      type: 'Feature', id: i, properties: { fictional: true, source: 'rendering benchmark' },
      geometry: { type: 'Point', coordinates: [-31 + (i % 100) * .04, 10 + Math.floor(i / 100) * .04] },
    }));
    let inherited = 0;
    try {
      const r = await fetch('/benchmark/inherited-geometries.json');
      if (!r.ok) throw new Error('Inherited geometry fixture unavailable');
      const geometry = await r.json() as GeoJSON.FeatureCollection;
      inherited = geometry.features.length;
      map.addSource('relic-benchmark-inherited', { type: 'geojson', data: geometry });
      map.addLayer({ id: 'relic-benchmark-inherited', type: 'circle', source: 'relic-benchmark-inherited', filter: ['==', '$type', 'Point'], paint: { 'circle-radius': 2, 'circle-color': '#af9274', 'circle-opacity': .7 } });
      map.addSource(sourceId, { type: 'geojson', data: { type: 'FeatureCollection', features } });
      map.addLayer({ id: sourceId, type: 'circle', source: sourceId, paint: { 'circle-radius': 2.5, 'circle-color': '#389f96', 'circle-opacity': .65 } });
    } catch (error) {
      for (const id of [sourceId, 'relic-benchmark-inherited']) { if (map.getLayer(id)) map.removeLayer(id); if (map.getSource(id)) map.removeSource(id); }
      setStatus(error instanceof Error ? error.message : 'Benchmark could not start'); return;
    }
    const startTime = performance.now();
    const heapStart = memory();
    const frames: number[] = [];
    let last = startTime, raf = 0, changed = 0, tick = 0, done = false;
    const duration = mode === 'normal' ? 20000 : 10000;
    const changes = mode === 'normal' ? 100 : 1000;
    let error: string | undefined;
    const canvas = map.getCanvas();
    const gl = canvas.getContext('webgl2');
    const renderer = gl?.getParameter(gl.RENDERER) as string | undefined;
    const frame = (time: number) => { frames.push(time - last); last = time; raf = requestAnimationFrame(frame); };
    raf = requestAnimationFrame(frame);
    const interval = window.setInterval(() => {
      try {
        const update = Array.from({ length: changes }, (_, j) => {
          const id = (tick * changes + j) % features.length;
          const point = features[id].geometry.coordinates;
          return { id, newGeometry: { type: 'Point' as const, coordinates: [point[0] + Math.sin(tick / 5) * .002, point[1] + Math.cos(tick / 5) * .002] } };
        });
        (map.getSource(sourceId) as GeoJSONSource).updateData({ update });
        changed += changes; tick++;
        if (tick % 20 === 0) map.easeTo({ center: [-29.2 + Math.sin(tick / 20) * .1, 12.1], zoom: 6.2 + .2 * Math.cos(tick / 20), duration: 600 });
      } catch (failure) { error = String(failure); stop.current?.(); }
    }, 100);
    const finish = () => {
      if (done) return; done = true;
      window.clearInterval(interval); window.clearTimeout(timeout); cancelAnimationFrame(raf);
      map.stop();
      const measured = {
        mode, elapsed_ms: Math.round(performance.now() - startTime), frames: frames.length,
        frame_p95_ms: Number(q(frames, .95).toFixed(2)), frame_max_ms: Number(Math.max(0, ...frames).toFixed(2)),
        stalls_over_100_ms: frames.filter(x => x > 100).length, submitted_changes: changed, contacts: 10000,
        inherited_geometry_count: inherited, heap_start_bytes: heapStart, heap_end_bytes: memory(),
        map_idle_at_stop: map.loaded(), browser: navigator.userAgent,
        viewport: { width: window.innerWidth, height: window.innerHeight, pixel_ratio: window.devicePixelRatio },
        request_origins: [...new Set(performance.getEntriesByType('resource').map(entry => { try { return new URL(entry.name).origin; } catch { return 'unresolved'; } }))],
        webgl_renderer: renderer ?? 'unavailable', ...(error ? { error } : {}),
      };
      setResult(measured);
      setStatus(error ? 'Stopped after error; inspect result' : 'Complete — rendering measurement, not source throughput');
      for (const id of [sourceId, 'relic-benchmark-inherited']) { if (map.getLayer(id)) map.removeLayer(id); if (map.getSource(id)) map.removeSource(id); }
      map.jumpTo(original);
      stop.current = null;
    };
    const timeout = window.setTimeout(finish, duration);
    stop.current = finish;
    setStatus(`${mode === 'normal' ? '1,000' : '10,000'} changes/second · ${duration / 1000} seconds · 10,000 synthetic contacts`);
    setResult(null);
  }

  return <aside aria-label="Rendering benchmark" style={{ position: 'fixed', bottom: 8, left: 12, zIndex: 999, maxWidth: 530, background: '#f5f2e9', color: '#1d302e', border: '1px solid #819a94', padding: 10, fontSize: 11, boxShadow: '0 2px 10px #0002' }}>
    <strong>Synthetic benchmark · no source/evidence writes</strong>
    <div role="status">{status}</div>
    <div style={{ display: 'flex', gap: 8, marginTop: 6 }}>
      <button onClick={() => void start('normal')}>Start normal load</button>
      <button onClick={() => void start('overload')}>Start overload burst</button>
      <button onClick={() => stop.current?.()}>Stop benchmark</button>
    </div>
    {result && <pre data-testid="benchmark-result" style={{ maxHeight: 180, overflow: 'auto', fontSize: 10, margin: '6px 0 0' }}>{JSON.stringify(result, null, 2)}</pre>}
    {interactions.length > 0 && <details><summary>Measured interface interactions ({interactions.length})</summary><pre data-testid="interaction-results" style={{ maxHeight: 100, overflow: 'auto', fontSize: 10 }}>{JSON.stringify(interactions)}</pre></details>}
  </aside>;
}
