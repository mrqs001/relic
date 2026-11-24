# Local globe place navigation

The map command palette uses [display/map-places.json](display/map-places.json), copied identically to `web/public/map-places.json` and served as `/map-places.json`. It contains **410 entries: 210 country/territory labels and 200 major/capital cities**. This is a local camera-navigation aid, separate from evidence, analytical scope and automatic geocoding.

Each row is:

```ts
{
  id: string;
  name: string;
  kind: 'country' | 'city';
  country?: string; // publisher country/territory label, for city disambiguation
  aliases: string[];
  longitude: number;
  latitude: number;
  zoom: number; // presentation preference, not precision or confidence
}
```

Search `name`, `aliases` and the optional city `country` label; selecting a result changes the camera only. Examples include Portugal/PT, Lisbon/Lisboa, United Kingdom/UK, New York City/New York, Tokyo and São Paulo. No network request is needed after the local index has loaded. This is not a complete gazetteer: a missing result does not establish absence.

## Source and selection

[places-manifest.json](places-manifest.json) records acceptance, byte counts, hashes, rights and limitations. [places-discovery.json](places-discovery.json) retains the actual official repository API discovery and commit-pinned download URLs. All three raw GeoJSON files and their product-version files are retained under `raw/`; the earlier land/coastline raw files and acceptance manifest were unchanged.

The source is the official [Natural Earth v5.1.2 repository release](https://github.com/nvkelso/natural-earth-vector/releases/tag/v5.1.2), commit `f1890d9f152c896d250a77557a5751a93d494776`, dated **13 May 2022**. Its country and tiny-country products identify themselves as **5.1.1**, and its populated-places product as **5.1.2**. These identify releases, not current observation dates. [Publisher cultural-data overview](https://www.naturalearthdata.com/downloads/110m-cultural-vectors/) describes these cartographic products.

- Countries: retain all 177 country records, then add 33 of 37 tiny-country points. The four overlaps are deduplicated solely by publisher `NE_ID`; the country layer supplies the retained label position. The enum `country` includes publisher-defined countries/territories and does not assert recognition or territorial control.
- Cities: sort the 243 publisher records by `SCALERANK` ascending, then `ADM0CAP` descending, then `POP_MAX` descending, then `NE_ID`. Retain the first 200, including 167 flagged by that source as capitals. The remaining 43 are an explicit size/coverage limit. The historic population field is used only to resolve display ranking/zoom and is not exposed as a current metric.
- City country labels retain each city's own `ADM0NAME` field, or are omitted when unavailable. They never join through country/tiny-country `ADM0_A3` codes: those codes can be shared by distinct labels, such as Portugal and Madeira. Sovereignty fields do not replace administrative labels. Lisbon/Lisboa therefore displays Portugal; Porto and Funchal are absent from this small-scale publisher source and are not added.
- Names: preserve publisher English display names and multilingual/source aliases. Country-code aliases retain their source values. Dotted initialisms also receive a compact typing variant, such as `U.K.` → `UK`. No current-name reconciliation or translated-name invention occurs.
- Coordinates: country entries use the publisher's `LABEL_X`/`LABEL_Y`, and cities retain their source GeoJSON points. Country zoom uses the largest source polygon's extent; tiny-country points use zoom 6 and cities use 6.2 or 7. These are camera choices, not analytical footprints or location accuracy.

Each derived ID contains the layer kind, pinned release and publisher `NE_ID`. [places-provenance.json](places-provenance.json) maps every output to its exact raw file, feature-array index, source ID and coordinate/name/country-label fields. Names and proximity never merge different source identities.

## Rebuild and validation

From the Relic project root:

```sh
node reference/accepted/natural-earth/build-map-places.mjs --write
node reference/accepted/natural-earth/build-map-places.mjs
```

The dependency-free build-time script runs offline. It verifies raw byte sizes, SHA-256 and publisher Git blob hashes; finite in-range coordinates; closed source polygon rings; unique IDs; explicit counts; every city's direct country-label attribution; representative names/aliases and Lisbon/Lisboa, London, Paris and New York country labels; and exact reproducibility of both index copies and the provenance/quality reports. [places-quality.json](places-quality.json) records the results. It is not an application import or a substitute for Rust evidence processing.

The existing pinned [public-domain licence](raw/LICENSE.md) applies; the [publisher terms](https://www.naturalearthdata.com/about/terms-of-use/) were checked. Display **Made with Natural Earth · v5.1.2** as a provenance aid. No other reference catalogue product or inherited input is enabled by this acceptance.
