// Offline build-time locator packaging; never an application evidence/geocoding importer.
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';

const root = new URL('./', import.meta.url);
const publicIndex = new URL('../../../web/public/map-places.json', root);
const read = path => readFileSync(new URL(path, root));
const json = path => JSON.parse(read(path));
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
const discovery = json('places-discovery.json');
assert.equal(discovery.commit, 'f1890d9f152c896d250a77557a5751a93d494776');
const sourceData = new Map();
for (const source of discovery.files) {
  const bytes = read(source.local_path);
  assert.equal(bytes.length, source.bytes);
  assert.equal(digest(bytes), source.sha256);
  assert.equal(createHash('sha1').update(`blob ${bytes.length}\0`).update(bytes).digest('hex'), source.git_blob_sha);
  assert.ok(source.download_url.includes(`/${discovery.commit}/`));
  if (source.local_path.endsWith('.geojson')) sourceData.set(source.local_path, JSON.parse(bytes));
}
const countryPath = 'raw/ne_110m_admin_0_countries.geojson';
const tinyPath = 'raw/ne_110m_admin_0_tiny_countries.geojson';
const cityPath = 'raw/ne_110m_populated_places.geojson';
assert.equal(sourceData.get(countryPath).features.length, 177);
assert.equal(sourceData.get(tinyPath).features.length, 37);
assert.equal(sourceData.get(cityPath).features.length, 243);
const compare = (a, b) => a < b ? -1 : a > b ? 1 : 0;
const key = value => value.normalize('NFKC').toLowerCase();
function text(value) {
  return typeof value === 'string' && value.trim() && value.trim() !== '-99' ? value.trim() : null;
}
function coordinate(point) {
  assert.equal(point.length, 2);
  assert.ok(point.every(Number.isFinite));
  assert.ok(point[0] >= -180 && point[0] <= 180);
  assert.ok(point[1] >= -90 && point[1] <= 90);
}
function names(p, name, extra) {
  const candidates = [...Object.keys(p).filter(k => /^NAME_[A-Z]{2,3}$/.test(k)), ...extra];
  const result = new Map();
  for (const field of candidates) {
    const label = text(p[field]);
    if (label && key(label) !== key(name)) result.set(key(label), label);
  }
  // Compact a supplied dotted initialism for typing (for example U.K. → UK).
  if (typeof p.ABBREV === 'string' && /^[A-Za-z](?:\.[A-Za-z])+\.?$/.test(p.ABBREV)) {
    const compact = p.ABBREV.replaceAll('.', '');
    result.set(key(compact), compact);
  }
  return [...result.values()].sort(compare);
}
const provenance = [];
const countries = new Map();
const duplicates = [];
let rings = 0;
let coordinates = 0;
for (const path of [countryPath, tinyPath]) {
  const collection = sourceData.get(path);
  assert.equal(collection.type, 'FeatureCollection');
  for (const [index, feature] of collection.features.entries()) {
    const p = feature.properties;
    assert.ok(Number.isSafeInteger(p.NE_ID) && p.NE_ID > 0);
    const sourceId = String(p.NE_ID);
    const point = [p.LABEL_X, p.LABEL_Y];
    coordinate(point);
    let largestArea = -1;
    let largestSpan = 1;
    if (feature.geometry.type === 'Point') {
      coordinate(feature.geometry.coordinates);
      coordinates += 1;
    } else {
      assert.ok(['Polygon', 'MultiPolygon'].includes(feature.geometry.type));
      const polygons = feature.geometry.type === 'Polygon' ? [feature.geometry.coordinates] : feature.geometry.coordinates;
      for (const polygon of polygons) {
        assert.ok(polygon.length > 0);
        for (const ring of polygon) {
          assert.ok(ring.length >= 4);
          assert.deepEqual(ring[0], ring.at(-1));
          rings += 1;
          for (const position of ring) { coordinate(position); coordinates += 1; }
        }
        const shell = polygon[0];
        let twiceArea = 0;
        for (let n = 1; n < shell.length; n++) twiceArea += shell[n - 1][0] * shell[n][1] - shell[n][0] * shell[n - 1][1];
        const area = Math.abs(twiceArea);
        if (area > largestArea) {
          largestArea = area;
          const xs = shell.map(v => v[0]), ys = shell.map(v => v[1]);
          largestSpan = Math.max(Math.max(...xs) - Math.min(...xs), (Math.max(...ys) - Math.min(...ys)) * 1.5);
        }
      }
    }
    if (countries.has(sourceId)) {
      duplicates.push({ ne_id: sourceId, source_path: path, source_feature_index: index, kept_path: countryPath });
      continue;
    }
    const name = text(p.NAME_EN) ?? text(p.NAME_LONG) ?? text(p.NAME);
    assert.ok(name);
    const record = {
      id: `natural-earth:v5.1.2:country:${sourceId}`, name, kind: 'country',
      aliases: names(p, name, ['NAME', 'NAME_LONG', 'NAME_ALT', 'NAME_SORT', 'NAME_CIAWF', 'ADMIN', 'FORMAL_EN', 'FORMAL_FR', 'ABBREV', 'POSTAL', 'ISO_A2', 'ISO_A3', 'ADM0_A3']),
      longitude: point[0], latitude: point[1],
      zoom: feature.geometry.type === 'Point' ? 6 : Number(Math.max(1.6, Math.min(6.5, Math.log2(360 / Math.max(largestSpan, 0.25)) - 0.3)).toFixed(2)),
    };
    countries.set(sourceId, record);
    provenance.push({ id: record.id, source_path: path, source_feature_index: index, publisher_ne_id: sourceId, coordinate_fields: ['LABEL_X', 'LABEL_Y'], name_field: text(p.NAME_EN) ? 'NAME_EN' : text(p.NAME_LONG) ? 'NAME_LONG' : 'NAME', publisher_adm0_a3: p.ADM0_A3 });
  }
}
assert.equal(countries.size, 210);
assert.equal(duplicates.length, 4);
const cities = sourceData.get(cityPath).features.map((feature, index) => {
  assert.equal(feature.geometry.type, 'Point');
  coordinate(feature.geometry.coordinates);
  coordinates += 1;
  const p = feature.properties;
  assert.ok(Number.isSafeInteger(p.NE_ID) && p.NE_ID > 0);
  assert.ok(Number.isFinite(p.SCALERANK) && Number.isFinite(p.POP_MAX));
  return { feature, index };
}).sort((a, b) => a.feature.properties.SCALERANK - b.feature.properties.SCALERANK
  || b.feature.properties.ADM0CAP - a.feature.properties.ADM0CAP
  || b.feature.properties.POP_MAX - a.feature.properties.POP_MAX
  || compare(String(a.feature.properties.NE_ID), String(b.feature.properties.NE_ID))).slice(0, 200);
const cityRows = cities.map(({ feature, index }) => {
  const p = feature.properties;
  const name = text(p.NAME_EN) ?? text(p.NAME);
  assert.ok(name);
  // ADM0_A3 is not a unique country-label key: Portugal and Madeira share PRT.
  // Retain the city's own administrative label; never infer it from another layer.
  const country = text(p.ADM0NAME);
  const record = {
    id: `natural-earth:v5.1.2:city:${p.NE_ID}`, name, kind: 'city',
    ...(country ? { country } : {}),
    aliases: names(p, name, ['NAME', 'NAMEASCII', 'NAMEALT', 'LS_NAME', 'MEGANAME']),
    longitude: feature.geometry.coordinates[0], latitude: feature.geometry.coordinates[1],
    zoom: p.POP_MAX >= 5_000_000 ? 6.2 : 7,
  };
  assert.equal(record.country, text(p.ADM0NAME) ?? undefined);
  provenance.push({ id: record.id, source_path: cityPath, source_feature_index: index, publisher_ne_id: String(p.NE_ID), coordinate_fields: ['geometry.coordinates[0]', 'geometry.coordinates[1]'], name_field: text(p.NAME_EN) ? 'NAME_EN' : 'NAME', country_field: country ? 'ADM0NAME' : null, publisher_adm0_a3: p.ADM0_A3, publisher_capital_flag: p.ADM0CAP });
  return record;
});
const records = [...countries.values(), ...cityRows].sort((a, b) => compare(a.name, b.name) || compare(a.kind, b.kind) || compare(a.id, b.id));
assert.equal(records.length, 410);
assert.equal(new Set(records.map(row => row.id)).size, records.length);
for (const row of records) {
  coordinate([row.longitude, row.latitude]);
  assert.ok(Number.isFinite(row.zoom) && row.zoom >= 1.6 && row.zoom <= 7);
  assert.ok(row.aliases.every(alias => typeof alias === 'string' && alias.length > 0));
}
for (const name of ['Portugal', 'Lisbon', 'London', 'New York', 'Tokyo', 'Singapore', 'São Paulo']) assert.ok(records.some(row => row.name === name || row.aliases.includes(name)), `Expected representative locator: ${name}`);
assert.ok(records.find(row => row.name === 'Lisbon').aliases.includes('Lisboa'));
assert.ok(records.find(row => row.name === 'Portugal').aliases.includes('PT'));
assert.ok(records.find(row => row.name === 'United Kingdom').aliases.includes('UK'));
const cityCountryChecks = [
  { query: 'Lisbon', country: 'Portugal' },
  { query: 'Lisboa', country: 'Portugal' },
  { query: 'Madrid', country: 'Spain' },
  { query: 'Oslo', country: 'Norway' },
  { query: 'London', country: 'United Kingdom' },
  { query: 'Paris', country: 'France' },
  { query: 'New York', country: 'United States of America' },
];
for (const expected of cityCountryChecks) {
  const row = cityRows.find(row => row.name === expected.query || row.aliases.includes(expected.query));
  assert.ok(row, `Expected city or alias: ${expected.query}`);
  assert.equal(row.country, expected.country, `Incorrect source administrative label for ${expected.query}`);
}
const missingSourcePlaces = ['Porto', 'Funchal'];
for (const place of missingSourcePlaces) {
  assert.ok(!sourceData.get(cityPath).features.some(({ properties }) => Object.entries(properties)
    .some(([field, value]) => /^NAME/.test(field) && typeof value === 'string' && key(value) === key(place))));
}
const bytes = Buffer.from(`${JSON.stringify(records)}\n`);
const quality = {
  schema_version: 1, outcome: 'passed_for_globe_place_navigation_only', commit: discovery.commit,
  count: records.length, countries: countries.size, cities: cities.length,
  publisher_flagged_capitals_selected: cities.filter(x => x.feature.properties.ADM0CAP === 1).length,
  source_counts: { countries: 177, tiny_country_points: 37, populated_places: 243 },
  duplicates_by_publisher_ne_id: duplicates,
  cities_omitted_by_rank_limit: 43,
  rejected_invalid_records: 0, closed_source_polygon_rings: rings, valid_source_coordinates: coordinates,
  bytes: bytes.length, sha256: digest(bytes),
  schema: '{id:string,name:string,kind:"country"|"city",country?:string,aliases:string[],longitude:number,latitude:number,zoom:number}[]',
  city_selection: 'SCALERANK ascending, ADM0CAP descending, POP_MAX descending, NE_ID lexicographic; first 200 of 243 publisher places.',
  country_selection: 'All 177 countries plus 33 additional tiny-country points; deduplicate only identical publisher NE_ID, preserving country-layer label where both exist.',
  city_country_policy: 'Retain each city source record ADM0NAME directly; omit the field if unavailable. Do not crosswalk ADM0_A3 through country/tiny-country layers or substitute sovereignty labels.',
  city_country_records_checked: cityRows.length,
  representative_city_country_checks: cityCountryChecks,
  requested_places_absent_from_source: missingSourcePlaces,
  coordinate_policy: 'Use publisher LABEL_X/LABEL_Y for country navigation, unchanged source GeoJSON point for cities; never infer report/evidence geography.',
  zoom_policy: 'Camera presentation preference only: country extent from largest polygon shell, clamped 1.6–6.5; tiny-country points 6; cities 6.2 for publisher POP_MAX>=5m else7. No population value is exposed as current evidence.',
  limitations: ['Pinned 2022 repository release; underlying label/position vintage may be older or unknown.', 'The country enum includes publisher-defined countries/territories, not a legal recognition assertion.', 'Not complete city/capital coverage; 43 source places omitted by explicit rank limit.', 'Aliases are publisher values, not reconciled current official names.', 'Label points and suggested zoom are for camera navigation only; never use them to locate evidence, test analytical scope or assert geography.'],
};
const outputs = new Map([
  [new URL('display/map-places.json', root), bytes],
  [publicIndex, bytes],
  [new URL('places-quality.json', root), Buffer.from(`${JSON.stringify(quality, null, 2)}\n`)],
  [new URL('places-provenance.json', root), Buffer.from(`${JSON.stringify(provenance.sort((a, b) => compare(a.id, b.id)), null, 2)}\n`)],
]);
if (process.argv.includes('--write')) {
  for (const [path, data] of outputs) { mkdirSync(new URL('./', path), { recursive: true }); writeFileSync(path, data); }
} else {
  for (const [path, data] of outputs) assert.deepEqual(readFileSync(path), data, `Derived locator drift: ${path.pathname}`);
  const manifest = json('places-manifest.json');
  assert.equal(manifest.source.commit, discovery.commit);
  assert.equal(manifest.status, 'accepted_for_globe_place_navigation_only');
  assert.equal(manifest.eligible_for_evidence, false);
  assert.equal(manifest.eligible_for_analytical_scope, false);
  assert.equal(manifest.eligible_for_automated_geocoding, false);
  assert.equal(manifest.index.sha256, digest(bytes));
  assert.equal(manifest.index.bytes, bytes.length);
  assert.equal(manifest.index.count, records.length);
  assert.equal(manifest.quality.sha256, digest(read('places-quality.json')));
  assert.equal(manifest.identity.provenance_sha256, digest(read('places-provenance.json')));
  assert.equal(manifest.rights.pinned_snapshot_sha256, digest(read('raw/LICENSE.md')));
}
console.log(JSON.stringify({ outcome: quality.outcome, count: records.length, countries: countries.size, cities: cities.length, bytes: bytes.length, sha256: quality.sha256, public_path: 'web/public/map-places.json' }));
