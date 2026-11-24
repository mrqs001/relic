// Explicit fixture import into a disposable local Relic workspace.
// node validation/seed_workload.mjs http://127.0.0.1:8791 state/workload
import assert from 'node:assert/strict';
import { readFile, writeFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { createHash } from 'node:crypto';

const base = new URL(process.argv[2] || 'http://127.0.0.1:8791');
assert.ok(['127.0.0.1', 'localhost', '[::1]'].includes(base.hostname), 'Workload tool only accepts loopback application targets');
const directory = resolve(process.argv[3] || 'state/workload');
const manifest = JSON.parse(await readFile(resolve(directory, 'manifest.json'), 'utf8'));
assert.equal(manifest.fictional, true);
const results = [];
for (const entry of manifest.files) {
  const bytes = await readFile(resolve(directory, entry.name));
  assert.equal(createHash('sha256').update(bytes).digest('hex'), entry.sha256);
  const started = performance.now();
  const response = await fetch(new URL('/api/v1/imports', base), {
    method: 'POST', headers: { 'Content-Type': 'application/json', Origin: base.origin }, body: bytes,
    signal: AbortSignal.timeout(60000),
  });
  const result = await response.json();
  assert.ok(response.ok, `${entry.name}: ${JSON.stringify(result)}`);
  results.push({ batch: entry.name, duration_ms: Math.round(performance.now() - started), accepted: result.accepted, reused: result.reused, publication_id: result.publication_id });
  if (results.length % 10 === 0) console.log(JSON.stringify({ imported_batches: results.length, last_ms: results.at(-1).duration_ms }));
}
await writeFile(resolve(directory, 'seed-results.json'), JSON.stringify({ timestamp: new Date().toISOString(), base: base.origin, batches: results }, null, 2));
console.log(JSON.stringify({ seeded: results.length, total_ms: results.reduce((n, x) => n + x.duration_ms, 0) }));
