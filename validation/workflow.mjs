// API invariant walkthrough against a FRESH disposable loopback workspace.
// The UI walkthrough is separate: this does not establish browser or performance acceptance.
import assert from 'node:assert/strict';
import { readFile, writeFile } from 'node:fs/promises';
const base = new URL(process.argv[2] || 'http://127.0.0.1:8792');
assert.ok(['127.0.0.1', 'localhost', '[::1]'].includes(base.hostname));
const checks = [];
async function api(path, method = 'GET', body, expected = 200) {
  const r = await fetch(new URL(`/api/v1${path}`, base), {
    method, headers: { 'Content-Type': 'application/json', Origin: base.origin },
    ...(body === undefined ? {} : { body: JSON.stringify(body) }), signal: AbortSignal.timeout(30000),
  });
  const text = await r.text();
  let result; try { result = JSON.parse(text); } catch { result = text; }
  if (expected === 'ok') assert.ok(r.ok, `${path}: ${r.status} ${text}`);
  else assert.equal(r.status, expected, `${path}: ${text}`);
  return result;
}
function pass(name) { checks.push({ name, result: 'passed' }); console.log(name); }
const health = await api('/health');
assert.equal(health.api_version, 1);
assert.equal((await api('/workspace')).empty, true, 'Use a fresh disposable workspace');
pass('Empty workspace and versioned health');
const imported = await api('/imports/demo', 'POST', undefined, 'ok');
const again = await api('/imports/demo', 'POST', undefined, 'ok');
assert.equal(imported.publication_id, again.publication_id);
pass('Idempotent demonstration import');
const workspace = await api('/workspace');
let investigation = workspace.investigations[0];
assert.ok(investigation);
const scope = { source_ids: [], include_unlocated: true, query: '', time_basis: 'publication_time', from: null, to: null, bbox: null, publication_id: null };
const query = async (s = scope, generation = 1) => api('/query', 'POST', { scope: s, generation, limit: 100 });
const current = await query();
assert.equal(current.total, 4);
assert.equal(current.located, 2);
assert.equal(current.unlocated, 2);
assert.equal(current.origin_groups, 2);
assert.equal(current.unknown_origins, 1);
assert.equal(current.map.features.length, 2);
assert.equal(current.generation, 1);
assert.ok(current.scope_revision && current.publication_id);
assert.equal(current.items.find(x => x.id === 'demo:report-1').revision, 'v2');
pass('Latest revision membership, map/list totals, origin and uncertainty counts');
const selected = await query({ ...scope, source_ids: ['demo:observer'] }, 2);
assert.equal(selected.total, 1);
assert.equal(selected.unlocated, 1);
assert.equal(selected.located, 0);
const hidden = await query({ ...scope, source_ids: ['demo:observer'], include_unlocated: false }, 3);
assert.equal(hidden.total, 0);
const searched = await query({ ...scope, query: 'witness' }, 4);
assert.equal(searched.total, 1);
assert.equal(searched.items[0].id, 'demo:report-4');
pass('Source, unlocated and lexical predicates use one scope');
const baseline = workspace.briefs.find(x => x.id === 'demo:brief-1');
assert.ok(baseline);
const old = await query({ ...scope, publication_id: baseline.publication_id }, 5);
assert.equal(old.total, 3);
assert.equal(old.located, 1);
assert.ok(!old.items.some(x => x.id === 'demo:report-3'));
const v1 = await api('/evidence?id=demo%3Areport-1&revision=v1');
const v2 = await api('/evidence?id=demo%3Areport-1&revision=v2');
assert.ok(v1.text.startsWith('Unconfirmed'));
assert.ok(v2.text.startsWith('Correction'));
assert.ok(v1.revisions.includes('v2'));
pass('Frozen baseline excludes late acquisitions and opens exact original revision');
const authority = current.items.find(x => x.id === 'demo:report-3');
const witness = current.items.find(x => x.id === 'demo:report-4');
const citations = [authority.citation, witness.citation, v2.citation];
const previousVersion = investigation.version;
investigation = await api(`/investigations/${encodeURIComponent(investigation.id)}`, 'PUT', {
  expected_version: previousVersion, notes: 'Alternative: a planned inspection explains closure; reported noise remains unresolved.',
  pins: [{ evidence_id: authority.id, revision: authority.revision, reason: 'Direct authority statement, compared with correction.' }],
});
assert.equal(investigation.notes.includes('unresolved'), true);
await api(`/investigations/${encodeURIComponent(investigation.id)}`, 'PUT', { expected_version: previousVersion, notes: 'stale write' }, 409);
pass('Durable notes, exact pins and stale-write conflict');
const claim = investigation.claims[0];
assert.ok(claim, 'Investigation must expose an independently identified claim');
investigation = await api(`/investigations/${encodeURIComponent(investigation.id)}/assessments`, 'POST', {
  expected_version: investigation.version, claim_id: claim.id, status: 'contested',
  rationale: 'Authority describes a planned inspection; no evidence establishes the cause of the noise.',
  citations, scope, publication_id: current.publication_id,
}, 'ok');
assert.equal(investigation.assessments.at(-1).status, 'contested');
pass('Assessment retained separately with rationale and exact support');
const briefBody = {
  title: 'Port Selene — assessed access disruption', investigation_id: investigation.id,
  question: investigation.question,
  finding: 'The authority describes a planned inspection. The local account corrected its report. The witness location remains unresolved.',
  assessment: 'contested', limitations: ['Cause of the reported noise is unestablished.', 'Witness location unresolved; fictional evidence only.'],
  citations, scope, publication_id: current.publication_id,
};
const brief = await api('/briefs', 'POST', briefBody, 'ok');
const opened = await api(`/briefs/${encodeURIComponent(brief.id)}?revision=${encodeURIComponent(brief.revision)}`);
assert.equal(opened.finding, brief.finding);
assert.deepEqual(opened.citations, citations);
const markdown = await api(`/briefs/${encodeURIComponent(brief.id)}/export?revision=${brief.revision}&format=markdown`);
assert.equal(typeof markdown, 'string');
assert.ok(markdown.includes(authority.text));
const manifest = await api(`/briefs/${encodeURIComponent(brief.id)}/export?revision=${brief.revision}&format=json`);
assert.ok(JSON.stringify(manifest).includes(witness.text));
pass('Saved/reopened brief and portable readable/structured exact evidence exports');
const badCitation = { ...authority.citation, revision: 'does-not-exist' };
await api('/briefs', 'POST', { ...briefBody, citations: [badCitation] }, 400);
await api('/briefs', 'POST', { ...briefBody, citations: [{ ...authority.citation, end_character: 999999 }] }, 400);
pass('Nonexistent and out-of-range citations rejected');
const lineage = await api(`/lineage?root=${encodeURIComponent(`${brief.id}@${brief.revision}`)}&depth=5&direction=both`);
assert.ok(lineage.nodes.length > 1);
assert.ok(lineage.edges.some(e => e.relationship === 'cited_by'));
assert.ok(lineage.nodes.some(n => n.id === `${authority.id}@${authority.revision}`));
pass('Brief lineage resolves actual cited revisions');
const watchBody = { title: 'Selene changes', question: 'What changes the access assessment?', scope, conditions: ['source correction', 'new observed origin'] };
const preview = await api('/watches/preview', 'POST', watchBody);
assert.equal(preview.frequency, 'unknown');
assert.equal((await api('/workspace')).watches.length, workspace.watches.length);
const watch = await api('/watches', 'POST', { ...watchBody, baseline_policy: 'fresh' }, 'ok');
assert.equal((await api('/alerts')).items.filter(x => x.watch_id === watch.id).length, 0);
pass('Watch preview is read-only; activation sets fresh baseline without history flood');
const fixture = JSON.parse(await readFile(new URL('../examples/port-investigation.json', import.meta.url), 'utf8'));
const original = fixture.evidence_revisions.find(x => x.id === 'demo:report-1' && x.revision === 'v2');
const updatedText = 'Correction update: the planned inspection is complete. The cause of the reported noise is still unknown.';
const update = { ...original, revision: 'v3', acquired_at: '2026-01-10T10:00:00Z', source_edited_at: '2026-01-10T09:59:00Z', text: updatedText,
  supersedes: { id: original.id, revision: 'v2' }, citation: { evidence_id: original.id, revision: 'v3', start_character: 0, end_character: [...updatedText].length } };
const bundle = { schema_version: 1, fictional: true, title: 'Later fictional correction', sources: fixture.sources, evidence_revisions: [update] };
await api('/imports', 'POST', bundle, 'ok');
const alerts = (await api('/alerts')).items.filter(x => x.watch_id === watch.id);
assert.ok(alerts.length > 0);
await api('/imports', 'POST', bundle, 'ok');
assert.equal((await api('/alerts')).items.filter(x => x.watch_id === watch.id).length, alerts.length);
await api(`/alerts/${encodeURIComponent(alerts[0].id)}`, 'PUT', { review_status: 'acknowledged' });
assert.equal((await api('/workspace')).investigations.find(x => x.id === investigation.id).assessments.at(-1).status, 'contested');
assert.equal((await api(`/briefs/${encodeURIComponent(brief.id)}?revision=${brief.revision}`)).finding, brief.finding);
pass('Correction alerts deduplicate; acknowledgement preserves assessment and saved brief');
const activeBeforeFailure = (await query()).publication_id;
await api('/imports', 'POST', { ...bundle, evidence_revisions: [{ ...update, id: 'demo:invalid', revision: 'v1', geometry: { type: 'Point', coordinates: [400, 95] } }] }, 400);
assert.equal((await query()).publication_id, activeBeforeFailure);
pass('Invalid required geometry cannot replace last complete publication');
const backup = await api('/backup', 'POST', undefined, 'ok');
const restored = await api(`/backup/${encodeURIComponent(backup.id)}/verify`, 'POST', undefined, 'ok');
assert.equal(restored.verified, true);
assert.ok(restored.briefs >= 2);
pass('Consistent snapshot restored into fresh directory with hashes, foreign keys and citations');
const report = { at: new Date().toISOString(), base: base.origin, sqlite_version: health.sqlite_version, checks, total: checks.length, outcome: 'passed', limitation: 'HTTP invariant checks only. UI, concurrent load and provider traffic are separate evidence.' };
const output = process.argv[3];
if (output) await writeFile(output, JSON.stringify(report, null, 2));
console.log(JSON.stringify(report));
