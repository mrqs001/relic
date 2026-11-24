// Offline packaging/validation of selected publisher locator assets. Not an application importer.
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';

const root = new URL('../reference/accepted/natural-earth/', import.meta.url);
const read = (path) => readFileSync(new URL(path, root));
const json = (path) => JSON.parse(read(path));
const sha256 = (bytes) => createHash('sha256').update(bytes).digest('hex');
const discovery = json('discovery.json');
assert.equal(discovery.tag, 'v5.1.2');
assert.equal(discovery.commit, 'f1890d9f152c896d250a77557a5751a93d494776');
assert.equal(read('raw/VERSION').toString().trim(), '5.1.2');
assert.ok(read('raw/LICENSE.md').toString().includes('# Everything here is public domain.'));
for (const source of discovery.selected_files) {
  const bytes = read(`raw/${source.path.split('/').at(-1)}`);
  assert.equal(bytes.length, source.bytes);
  assert.equal(sha256(bytes), source.sha256);
  assert.equal(createHash('sha1').update(`blob ${bytes.length}\0`).update(bytes).digest('hex'), source.git_blob_sha);
  assert.ok(source.download_url.includes(`/${discovery.commit}/`));
}

const expected = {
  land: { features: 127, rings: 128, coordinates: 5143, geometry: 'Polygon' },
  coastline: { features: 134, rings: 0, coordinates: 5128, geometry: 'LineString' },
};
const reports = [];
const derived = new Map();
for (const [layer, target] of Object.entries(expected)) {
  const rawPath = `raw/ne_110m_${layer}.geojson`;
  const bytes = read(rawPath);
  assert.ok(bytes.length < 2 * 1024 * 1024);
  const collection = JSON.parse(bytes);
  assert.equal(collection.type, 'FeatureCollection');
  assert.equal(collection.features.length, target.features);
  assert.equal(collection.crs.properties.name, 'urn:ogc:def:crs:OGC:1.3:CRS84');
  let rings = 0;
  let coordinates = 0;
  const bounds = [Infinity, Infinity, -Infinity, -Infinity];
  for (const feature of collection.features) {
    assert.equal(feature.type, 'Feature');
    assert.equal(feature.geometry.type, target.geometry);
    const lines = target.geometry === 'Polygon' ? feature.geometry.coordinates : [feature.geometry.coordinates];
    assert.ok(lines.length > 0);
    for (const line of lines) {
      assert.ok(line.length >= (target.geometry === 'Polygon' ? 4 : 2));
      if (target.geometry === 'Polygon') {
        rings += 1;
        assert.deepEqual(line[0], line.at(-1), 'Unclosed polygon ring');
      }
      for (const point of line) {
        coordinates += 1;
        assert.equal(point.length, 2);
        assert.ok(point.every(Number.isFinite));
        assert.ok(point[0] >= -180 && point[0] <= 180);
        assert.ok(point[1] >= -90 && point[1] <= 90);
        bounds[0] = Math.min(bounds[0], point[0]);
        bounds[1] = Math.min(bounds[1], point[1]);
        bounds[2] = Math.max(bounds[2], point[0]);
        bounds[3] = Math.max(bounds[3], point[1]);
      }
    }
  }
  assert.equal(rings, target.rings);
  assert.equal(coordinates, target.coordinates);
  const display = {
    type: 'FeatureCollection',
    bbox: bounds,
    features: collection.features.map((feature, index) => ({
      ...feature,
      id: `natural-earth:${discovery.tag}:ne_110m_${layer}:row-${index}`,
    })),
  };
  assert.equal(new Set(display.features.map((feature) => feature.id)).size, target.features);
  display.features.forEach((feature, index) => {
    assert.deepEqual(feature.geometry, collection.features[index].geometry);
    assert.deepEqual(feature.properties, collection.features[index].properties);
  });
  const encoded = Buffer.from(`${JSON.stringify(display)}\n`);
  derived.set(`display/${layer}.geojson`, encoded);
  reports.push({
    layer, raw_path: rawPath, raw_bytes: bytes.length, raw_sha256: sha256(bytes),
    feature_count: target.features, geometry_type: target.geometry,
    coordinate_count: coordinates, closed_polygon_rings: rings,
    nonfinite_coordinates: 0, out_of_range_coordinates: 0, unclosed_polygon_rings: 0,
    rejected_records: 0, excluded_records: 0, coordinate_order: 'longitude, latitude',
    coordinate_reference_system: 'OGC CRS84 (WGS84 degrees)',
    coordinate_bbox: bounds, publisher_bbox: collection.bbox,
    publisher_bbox_matches_coordinates: JSON.stringify(bounds) === JSON.stringify(collection.bbox),
    display_path: `display/${layer}.geojson`, display_bytes: encoded.length,
    display_sha256: sha256(encoded),
  });
}
const quality = {
  schema_version: 1,
  outcome: 'passed_for_small_scale_locator_display',
  validator: 'validation/check_natural_earth.mjs',
  source_commit: discovery.commit,
  checks: ['publisher byte count', 'SHA-256', 'Git blob SHA-1', 'CRS', 'geometry types', 'coordinate shape/range/finiteness', 'closed polygon rings', 'release-local identity uniqueness', 'unchanged display coordinates and properties'],
  identity: 'The raw GeoJSON has no stable per-feature publisher IDs. Derived IDs qualify zero-based raw feature rows by dataset and pinned release. They are display IDs, not cross-release physical entities.',
  transformation: 'Preserve every feature, coordinate and property; add release-local display IDs, omit legacy top-level crs/name metadata and recompute display bbox from actual coordinates. Raw bytes remain unchanged.',
  findings: ['The coastline publisher bbox carries finer precision than the rounded coordinates, including east 180.00000044181. Actual coordinates are within range; the derived bbox uses those actual coordinates.', 'One coastline feature has publisher featurecla=Country. It is retained as publisher metadata and does not create a political or analytical boundary assertion.'],
  limitations: ['Generalized 1:110m locator geometry; not navigation or surveyed detail.', 'Coordinate/ring checks do not prove topology, world accuracy or completeness.', 'No legal/administrative membership, geocoding, incident association or evidence support may use this display asset.', 'Release date is known; per-feature real-world observation dates are not supplied.'],
  datasets: reports,
};
const qualityBytes = Buffer.from(`${JSON.stringify(quality, null, 2)}\n`);
derived.set('quality.json', qualityBytes);
if (process.argv.includes('--write-derived')) {
  mkdirSync(new URL('display/', root), { recursive: true });
  for (const [path, bytes] of derived) writeFileSync(new URL(path, root), bytes);
} else {
  for (const [path, bytes] of derived) assert.deepEqual(read(path), bytes, `Derived file differs: ${path}`);
  const manifest = json('manifest.json');
  assert.equal(manifest.source.commit, discovery.commit);
  assert.equal(manifest.status, 'accepted_for_local_locator_display');
  assert.equal(manifest.eligible_for_analysis, false);
  assert.equal(manifest.eligible_as_evidence, false);
  assert.equal(manifest.quality.report_sha256, sha256(qualityBytes));
  for (const asset of manifest.assets) {
    assert.equal(sha256(read(asset.raw_path)), asset.raw_sha256);
    assert.equal(sha256(read(asset.display_path)), asset.display_sha256);
    assert.equal(read(asset.raw_path).length, asset.raw_bytes);
    assert.equal(read(asset.display_path).length, asset.display_bytes);
  }
}
console.log(JSON.stringify({ outcome: quality.outcome, source_commit: discovery.commit, raw_bytes: reports.reduce((n, row) => n + row.raw_bytes, 0), features: reports.reduce((n, row) => n + row.feature_count, 0), closed_polygon_rings: 128, coordinates: 10271, display_coordinates_changed: 0 }));
