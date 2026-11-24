// Offline public-handoff checks. No dependencies, configuration loading, or network.
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { readFileSync, writeFileSync } from 'node:fs';

const root = new URL('../', import.meta.url);
const read = (path) => JSON.parse(readFileSync(new URL(path, root), 'utf8'));
const fixture = read('examples/port-investigation.json');
assert.equal(fixture.fictional, true);
const sources = new Set(fixture.sources.map((source) => source.id));
assert.equal(sources.size, fixture.sources.length);
const key = (record) => `${record.evidence_id ?? record.id}@${record.revision}`;
const revisions = new Map(fixture.evidence_revisions.map((record) => [key(record), record]));
assert.equal(revisions.size, fixture.evidence_revisions.length);
const citation = (reference) => {
  const record = revisions.get(key(reference));
  assert.ok(record, `Missing citation revision ${key(reference)}`);
  assert.ok(Number.isInteger(reference.start_character) && Number.isInteger(reference.end_character));
  assert.ok(reference.start_character >= 0 && reference.start_character < reference.end_character);
  assert.ok(reference.end_character <= Array.from(record.text).length, `Citation outside text: ${key(reference)}`);
};
for (const record of revisions.values()) {
  assert.ok(record.id.startsWith('demo:'));
  assert.equal(record.fictional, true);
  assert.ok(sources.has(record.source));
  citation(record.citation);
  for (const relation of [record.copies_from, record.supersedes].filter(Boolean)) {
    assert.ok(revisions.has(key(relation)));
  }
}
const brief = fixture.baseline_brief;
brief.citations.forEach(citation);
assert.deepEqual(brief.scope, fixture.investigation.scope);
const frozen = brief.input_snapshot.evidence_revisions.map((reference) => {
  const record = revisions.get(key(reference));
  assert.ok(record);
  assert.ok(Date.parse(record.acquired_at) <= Date.parse(brief.knowledge_cutoff));
  return record;
});
assert.equal(frozen.length, 3);
assert.equal(frozen.filter((record) => record.geometry !== null).length, 1);
assert.equal(frozen.filter((record) => record.geometry === null).length, 2);
assert.equal(frozen.filter((record) => record.origin_group === 'demo:origin-1').length, 2);
assert.equal(frozen.filter((record) => record.origin_group === null && record.dependence === 'unknown').length, 1);
const graph = fixture.lineage_example;
assert.equal(graph.fictional, true);
const nodes = new Map(graph.nodes.map((node) => [node.id, node]));
assert.equal(nodes.size, graph.nodes.length);
for (const edge of graph.edges) {
  assert.ok(nodes.has(edge.source) && nodes.has(edge.target), 'Dangling lineage edge');
  if (edge.citation) citation(edge.citation);
  if (edge.evidence_revision) assert.ok(revisions.has(key(edge.evidence_revision)));
}
graph.record_trace.path.forEach((id, index, path) => {
  assert.ok(nodes.has(id));
  if (index) assert.ok(graph.edges.some((edge) => edge.source === path[index - 1] && edge.target === id));
});
assert.equal(nodes.get('demo:map-list-1').eligible_reports, frozen.length);
assert.equal(nodes.get('demo:map-list-1').placed_reports, 1);
assert.equal(nodes.get('demo:map-list-1').unlocated_reports, 2);
const catalogue = read('reference/catalogue.json');
assert.equal(catalogue.datasets.length, 51);
assert.equal(new Set(catalogue.datasets.map((dataset) => dataset.id)).size, 51);
assert.ok(catalogue.datasets.every((dataset) => dataset.default_enabled === false));
assert.equal(catalogue.research_domains.length, 10);

console.log(JSON.stringify({
  outcome: 'passed', fictional_revisions: revisions.size, sources: sources.size,
  baseline_reports: frozen.length, baseline_placed: 1, baseline_unlocated: 2,
  lineage_nodes: nodes.size, lineage_edges: graph.edges.length,
  catalogue_candidates: catalogue.datasets.length, candidate_defaults_enabled: 0,
}));
