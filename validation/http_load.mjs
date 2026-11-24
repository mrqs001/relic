// Bounded concurrent HTTP measurement on the explicitly seeded fictional workspace.
import assert from 'node:assert/strict';
import { readFile, writeFile } from 'node:fs/promises';
import os from 'node:os';
const base = new URL(process.argv[2] || 'http://127.0.0.1:8791');
assert.ok(['127.0.0.1', 'localhost', '[::1]'].includes(base.hostname));
const observations = [];
async function api(path, method = 'GET', body) {
  const start = performance.now();
  const response = await fetch(new URL(`/api/v1${path}`, base), {
    method, headers: { 'Content-Type': 'application/json', Origin: base.origin },
    ...(body === undefined ? {} : { body: JSON.stringify(body) }), signal: AbortSignal.timeout(30000),
  });
  const headersAt = performance.now();
  const bytes = await response.text();
  const bodyAt = performance.now();
  const value = JSON.parse(bytes);
  observations.push({ path, status: response.status, to_headers_ms: headersAt - start, body_ms: bodyAt - headersAt, parse_ms: performance.now() - bodyAt, total_ms: performance.now() - start, bytes: Buffer.byteLength(bytes) });
  return { status: response.status, value };
}
const workspace = (await api('/workspace')).value;
assert.ok(workspace.sources.length >= 20 && workspace.sources.every(s => s.fictional), 'Use a disposable fully fictional seeded workspace');
const health = (await api('/health')).value;
const scope = { source_ids: [], include_unlocated: true, query: 'selene-workload', time_basis: 'publication_time', from: null, to: null, bbox: null, publication_id: null };
const initial = (await api('/query', 'POST', { scope, generation: 0, limit: 100 })).value;
assert.equal(initial.total, 10000);
assert.equal(initial.located, 8000);
assert.equal(initial.unlocated, 2000);
const record = initial.items[0];
const first = JSON.parse(await readFile(new URL('../state/workload/batch-000.json', import.meta.url), 'utf8'));
const importBundle = structuredClone(first);
importBundle.title = 'Concurrent cancellation test candidate';
importBundle.evidence_revisions = Array.from({ length: 2000 }, (_, n) => {
  const e = structuredClone(first.evidence_revisions[n % 1000]);
  e.id = `stress:cancel-${n}`; e.citation.evidence_id = e.id; e.text = e.text.replace('selene-workload', 'cancel-workload');
  e.citation.end_character = [...e.text].length; return e;
});
let importDone = false, importFinishedAt = 0, stopAcknowledgedAt = 0;
const importStart = performance.now();
const bulk = api('/imports', 'POST', importBundle).finally(() => { importDone = true; importFinishedAt = performance.now(); });
let cancelResult = null;
const cancellation = (async () => {
  for (let attempt = 0; attempt < 25 && !importDone; attempt++) {
    const jobs = (await api('/jobs')).value.items;
    const job = jobs.find(x => x.kind === 'import' && x.status === 'running');
    if (job) {
      const start = performance.now();
      const result = await api(`/jobs/${encodeURIComponent(job.id)}/cancel`, 'POST');
      stopAcknowledgedAt = performance.now();
      cancelResult = { status: result.status, state: result.value.status, feedback_ms: performance.now() - start, import_ms_before_stop: start - importStart };
      return;
    }
    await new Promise(resolve => setTimeout(resolve, 10));
  }
})();
for (let round = 0; round < 12; round++) {
  const values = await Promise.all([
    api('/query', 'POST', { scope: { ...scope, query: round % 2 ? 'selene-workload' : 'witness-no-match' }, generation: round + 1, limit: 100 }),
    api(`/evidence?id=${encodeURIComponent(record.id)}&revision=${encodeURIComponent(record.revision)}`),
    api('/health'),
  ]);
  values.forEach(x => assert.equal(x.status, 200));
}
await cancellation;
const finished = await bulk;
const final = (await api('/query', 'POST', { scope, generation: 99, limit: 100 })).value;
assert.equal(final.total, 10000);
if (finished.status === 409) assert.equal(final.publication_id, initial.publication_id);
const p95 = xs => [...xs].sort((a, b) => a - b)[Math.min(xs.length - 1, Math.floor(xs.length * .95))];
const grouped = Object.fromEntries(['/query', '/evidence', '/health'].map(path => {
  const rows = observations.filter(x => x.path.split('?')[0] === path);
  return [path, { samples: rows.length, total_p95_ms: Number(p95(rows.map(x => x.total_ms)).toFixed(2)), headers_p95_ms: Number(p95(rows.map(x => x.to_headers_ms)).toFixed(2)), parse_p95_ms: Number(p95(rows.map(x => x.parse_ms)).toFixed(2)), max_bytes: Math.max(...rows.map(x => x.bytes)) }];
}));
const report = {
  timestamp: new Date().toISOString(), environment: { platform: os.platform(), release: os.release(), cpu: os.cpus()[0].model, logical_cpus: os.cpus().length, total_memory_bytes: os.totalmem(), node: process.version },
  workload: { report_revisions: 50000, sources: 20, matching_reports: 10000, located: 8000, unlocated: 2000, concurrent_import_candidate: 2000 },
  limits: health.limits, measured: grouped, cancellation: { ...cancelResult, final_http_status: finished.status, final_state: finished.value.error?.code || 'completed', total_import_ms: Math.round(importFinishedAt - importStart), worker_stopped_after_ack_ms: stopAcknowledgedAt ? Math.max(0, Math.round(importFinishedAt - stopAcknowledgedAt)) : null },
  outcome: 'measured', limitations: ['HTTP latency does not include browser parsing/rendering.', 'No real source or slow provider was used.', 'Normal/overload live presentation measurement is separate.', 'No source throughput or complete release acceptance claim.'],
};
if (process.argv[3]) await writeFile(process.argv[3], JSON.stringify(report, null, 2));
console.log(JSON.stringify(report, null, 2));
