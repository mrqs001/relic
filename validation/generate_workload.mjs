// Deterministic fictional workload; a build/validation utility, never a source runtime.
// node validation/generate_workload.mjs [output-directory]
import { mkdir, writeFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { createHash } from 'node:crypto';

const out = resolve(process.argv[2] || 'state/workload');
await mkdir(out, { recursive: true });
const sources = Array.from({ length: 20 }, (_, i) => ({
  id: `stress:source-${String(i).padStart(2, '0')}`,
  label: `Fictional workload source ${i + 1}`,
  family: i % 5 === 0 ? 'institutional_advisory' : 'authored_report',
  fictional: true,
}));
const base = Date.parse('2026-01-10T08:00:00Z');
const records = [];
for (let n = 0; n < 50000; n++) {
  const edited = n >= 40000;
  const logical = edited ? n - 40000 : n;
  const source = sources[logical % 20];
  const language = ['en', 'pt', 'ar', 'zh', 'uk'][logical % 5];
  const phrase = ['No cause established.', 'A causa não foi estabelecida.', 'لم يتم تحديد السبب.', '原因尚未确定。', 'Причину не встановлено.'][logical % 5];
  const text = `${logical < 10000 ? 'selene-workload' : 'other-workload'} ${edited ? 'Correction' : 'Report'} ${logical}: Fictional inspection notice. ${phrase} ${logical % 7 === 0 ? 'Repeated upstream statement; this is not independent confirmation.' : 'Attribution is a fictional test source.'}`;
  const id = `stress:report-${String(logical).padStart(5, '0')}`;
  const revision = edited ? 'v2' : 'v1';
  const coordinates = logical % 9 === 0 ? [-29.2, 12.1] : [-31 + (logical % 400) / 100, 10 + (Math.floor(logical / 400) % 400) / 100];
  records.push({
    id, revision, source: source.id,
    published_at: new Date(base + logical * 1000).toISOString(),
    acquired_at: new Date(base + n * 1000 + 120000).toISOString(),
    ...(edited ? { source_edited_at: new Date(base + n * 1000).toISOString(), supersedes: { id, revision: 'v1' } } : {}),
    text, geometry: logical % 5 === 0 ? null : { type: 'Point', coordinates },
    location_precision: logical % 5 === 0 ? 'unresolved' : 'approximate_locality',
    origin_group: logical % 11 === 0 ? null : `stress:origin-${Math.floor(logical / 3)}`,
    ...(logical % 11 === 0 ? { dependence: 'unknown' } : {}),
    language, fictional: true, availability: 'available',
    citation: { evidence_id: id, revision, start_character: 0, end_character: [...text].length },
    ...(logical % 100 === 0 ? { media: [{ availability: 'reference-only', bytes: 128000000, uri: `urn:fictional:media:${logical}`, meaning: 'Large fictional reference only; no network URL or body.' }] } : {}),
  });
}
const files = [];
for (let i = 0; i < records.length; i += 1000) {
  const name = `batch-${String(i / 1000).padStart(3, '0')}.json`;
  const body = JSON.stringify({ schema_version: 1, fictional: true, title: `Fictional load batch ${i / 1000}`, sources, evidence_revisions: records.slice(i, i + 1000) });
  await writeFile(resolve(out, name), body);
  files.push({ name, bytes: Buffer.byteLength(body), sha256: createHash('sha256').update(body).digest('hex') });
}
const manifest = {
  schema_version: 1, fictional: true, seed: 'relic-load-v1', source_count: 20,
  revisions: 50000, logical_reports: 40000, matching_query: 'selene-workload', matching_reports: 10000,
  matching_located: 8000, matching_unlocated: 2000,
  features: ['Unicode spans', 'coincident locations', 'unlocated reports', 'revisions', 'unknown origins', 'repeated origins', 'large media references without downloads'],
  live_layer: { contacts: 10000, normal_changes_per_second: 1000, burst_changes_per_second: 10000, burst_seconds: 10, evidence: false },
  files,
};
await writeFile(resolve(out, 'manifest.json'), JSON.stringify(manifest, null, 2));
console.log(JSON.stringify({ output: out, batches: files.length, bytes: files.reduce((n, x) => n + x.bytes, 0), ...Object.fromEntries(['revisions', 'logical_reports', 'matching_reports'].map(k => [k, manifest[k]])) }));
