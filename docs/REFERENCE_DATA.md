# Reference data

Relic resolves places only against reference data that was acquired explicitly, validated and activated as one version. A failed rebuild leaves the previous version in place. Each import records its input URL, retrieval time, checksum, licence, coverage, vintage and rejected counts.

## Geographic references

Use [reference/DATA_PLAN.md](../reference/DATA_PLAN.md) for dataset selection and [reference/catalogue.json](../reference/catalogue.json) for publisher/access locations, identities, rights and limitations across ten researched domains. These are acquisition candidates, not installed or fully validated payloads. Build references explicitly in the background, validate them, then activate one version atomically. A failed rebuild leaves the old version available. Record each input URL/file, retrieval time, checksum, licence, coverage, source vintage, rejected counts and resolver version. Show changes before replacing analyst corrections. No paid geocoding service is required.

- **GeoNames:** download `https://download.geonames.org/export/dump/allCountries.zip` and `https://download.geonames.org/export/dump/alternateNamesV2.zip`; extract only their expected text members under size limits. [The format description](https://download.geonames.org/export/dump/readme.txt) defines IDs, names, coordinates, feature class/code, countries, administrative codes and language-tagged aliases. Preserve the stable GeoNames ID, native names and hierarchy. Do not restrict all resolution to major cities: settlements, administrative areas and facilities have different meanings. Licence attribution accompanies exports.
- **Airports:** `https://davidmegginson.github.io/ourairports-data/airports.csv`; preserve `id`, `ident`, `type`, `name`, `latitude_deg`, `longitude_deg`, `iso_country`, `iso_region`, `municipality`, `gps_code`, `iata_code`, `local_code`, and source version. [OurAirports](https://ourairports.com/data/) supplies definitions and public-domain notice. Airport codes are useful aliases; they are not evidence of military purpose.
  The global archives are the complete catalogue contract, not a requirement to acquire the entire world before publication. The current bounded Rust acquisition path explicitly selects `cities500.zip` and/or `{CC}.zip`, with language-tagged country aliases at `alternatenames/{CC}.zip`, from the same official dump directory. Raw archives, expected text members, publisher receipts and hashes are retained. Global cities500 supplies primary/ASCII locality names; it does not supply full global multilingual aliases. Its flattened alternate-name field has no language/history flags. Rebuilt indexes may use it only to detect competing identities and verify global name coverage for an independently accepted regional match, never to create a positive alias match or coordinates. Keep these possible names separate from active language-tagged aliases; raw-source hashes and bounded candidate counts remain inspectable. Blank publisher feature codes are retained as unclassified context and counted in the manifest; they cannot produce occurrence geometry. Full country extracts retain other feature classes as context; only unambiguous populated places can propose approximate occurrence points. Activation records the exact selected products and coverage. This is a bounded staged acquisition choice, not a replacement for broader area/facility coverage.

- **Administrative polygons:** `GET https://www.geoboundaries.org/api/current/gbOpen/{ISO3}/ADM1/`, then follow `gjDownloadURL` for the full GeoJSON used by analytical geometry predicates. Keep `simplifiedGeometryGeoJSON` separately for display where appropriate; display simplification cannot change analytical membership. Store geometry and metadata such as boundary source/year/licence together. `shapeID`, `shapeName` and GeoNames administrative codes are different namespaces. Name matching can propose a crosswalk, but ambiguity must abstain. Choose watched countries explicitly; do not hardcode the old regional list. [Provider schema](https://www.geoboundaries.org/api.html).
- **OSM enrichment:** the selected input is regional **Geofabrik `.osm.pbf` extracts**, acquired for explicitly chosen areas. Preserve raw OSM identity, tags and geometry; follow the acquisition contract below. This does not require Nominatim, an OSM account, the whole planet or the former extraction scripts.

### Regional Geofabrik PBF acquisition

This is the implementation contract for `geofabrik-osm-regional-extracts` in the source catalogue. Documentation and discovery metadata were checked on 10 September 2026; no PBF payload or importer was tested.

**Discover:** use `https://download.geofabrik.de/index-v1-nogeom.json` for a small region selector, or `https://download.geofabrik.de/index-v1.json` for coverage polygons. Both are GeoJSON FeatureCollections. Preserve region `id`, `parent`, `name`, ISO-code arrays where supplied and `urls.pbf`; `urls.updates` locates replication state. Select public `pbf`, not account-only `pbf-internal` or `history`. Extract polygons describe extraction coverage rather than legal borders. [Publisher discovery index](https://download.geofabrik.de/index-v1.json).

**Concrete example:** the [Portugal catalogue](https://download.geofabrik.de/europe/portugal.html) links `https://download.geofabrik.de/europe/portugal-latest.osm.pbf`, `https://download.geofabrik.de/europe/portugal.poly` and `https://download.geofabrik.de/europe/portugal-updates/`. Portugal is an example, not a default region. Prefer URLs from discovery instead of constructing paths from country names. The catalogue also exposes dated snapshots/checksums; availability is not a permanent archive guarantee.

**Acquire and preserve:** download one selected region in a bounded background job to staging. Save region/index revision, resolved asset URL, actual byte count, acquisition time, SHA-256, publisher checksum when supplied, PBF header/replication metadata and extraction polygon. Distinguish data cutoff from download time and mutable `latest` naming. Verify the complete file before acceptance. Public PBFs omit contributor `user`, `uid` and `changeset`; ordinary snapshots do not contain full edit history. [Geofabrik format/metadata notes](https://download.geofabrik.de/technical.html).

**Decode and normalize:** treat PBF as a graph of nodes, ways and relations. Resolve node/member references before assembling lines, polygons and multipolygons. Keep required untagged supporting nodes and relation members when filtering named/relevant features; filtering them too early can destroy geometry. A closed way is not automatically an area. Record unsupported or unresolved geometry and retain its raw identity instead of inventing a centroid. Imported coordinates use OSM geographic coordinates; retain complete geometry for analysis and derive separately identified display geometry.

Retain `osm_type`, string `osm_id`, available object version/timestamp, raw tags, source snapshot, geometry lineage and quality flags. Preserve `name`, language-specific names, alternative/old names and operator/reference identifiers as supplied. A versioned mapping can derive Relic classes from tags such as `place`, `amenity`, `building`, `highway`, `railway`, `aeroway`, `power`, `landuse` and `waterway`. These are mapping inputs, not a claim that every tagged object is named, current or accurately classified. GIS-export fields such as `fclass` are not native PBF fields. Multiple feature roles may share one source object without becoming multiple independent entities.

**Overlaps and coverage:** deduplicate the same OSM type/ID/version across extracts while preserving each acquisition's membership. If snapshots disagree, retain revisions and use an explicit active-version rule. Never merge different objects by name/proximity alone. Geofabrik retains complete crossing ways/multipolygons, so objects can extend beyond the extraction polygon; clip only a derived view. Removing an object from a changed extraction footprint does not prove that it was deleted from OSM. [Extraction semantics](https://download.geofabrik.de/technical.html).

**Refresh:** begin with full regional snapshot rebuilds and atomic replacement after quality review. Add incremental updates only when download/import measurements justify them. Use the selected extract's replication URL/sequence, apply a contiguous chain durably and advance its checkpoint after success. Never apply planet or another region's diffs to it. Geofabrik currently retains diffs for 100 days; a missing/expired chain requires a fresh snapshot. Diffs can also reflect extraction-boundary changes. [Replication contract](https://download.geofabrik.de/technical.html).

**Publish and verify:** apply the data plan's provenance/rights checks, including [OSM attribution and ODbL](https://www.openstreetmap.org/copyright). Build spatial and name/alias indexes for local lookup, nearby context and spatial association. Keep the previous accepted revision on failure and preserve analyst notes/citations. Demonstrate a tiny regional import, multilingual aliases, a way dependent on untagged nodes, a multipolygon with holes, overlapping extracts, failed refresh and scope removal without a false deletion. Record missing-member/rejected counts, peak memory, cancellation behavior and actual served coverage. Use Rust for the new application import/processing work under the architecture requirement; this contract requires no separate extraction runtime or Nominatim service.

### Selected SeaVoX marine areas

Product: Marine Regions / VLIZ **SeaVoX v19 (2023)**; official WFS
`https://geo.vliz.be/geoserver/MarineRegions/wfs`. This is separate from maritime
jurisdiction/EEZ products. [Publisher services](https://www.marineregions.org/webservices.php),
[downloads](https://www.marineregions.org/downloads.php),
[terms and attribution](https://www.marineregions.org/disclaimer.php).

Explicit first selection: Strait of Hormuz, SeaVoX sub-region MRGID **24076**,
and Arabian Sea, MRGID **24074**. The gazetteer may prefer another classification;
those preferred IDs must not replace these layer identities. Parameters: WFS 2.0.0
GetFeature, `typeNames=MarineRegions:seavox_v19`, numeric equality `mrgid_sr=…`,
`count=2`, `outputFormat=application/json`, `srsName=EPSG:4326`. Exactly one matched,
returned and total feature is required for each selection. Retain exact response,
request/time, checksum, publisher name, vocabulary URI, MRGID and selection/rights
review in CAS. Coordinate order is longitude/latitude and bounds must agree with
publisher properties within their printed precision.

Each response has a 16-MiB/45-second bound, no redirects, no implicit proxy settings,
and cancellable acquisition. Activation validates all selected assets before an
atomic pointer change. Full geometry is also the initial display geometry; no
simplification/repair alters analytical membership. Marine-only topology admission
allows isolated ring tangencies with an acyclic contact graph; crossings, shared
edges, nested holes and contacts disconnecting the interior reject. Vertex and
intersection-work bounds remain. Historical shapes survive replacement.

Names/Arabic/Persian aliases in `reference/marine-selections.json` are explicit Relic
review decisions, not claimed publisher translations. Canonical-only guesses,
generic sea/strait labels and conflicting accepted marine names cannot select an
area. Reported event roles may receive area precision; issuer/destination/planned/
denied mentions cannot. Country hints never narrow a transboundary sea to a country.
Attribution is visible on the map. CC BY 4.0; geographic research reference only,
not legal, resource-exploration or navigational authority. Raw acquired files stay
private; refer users to Marine Regions for publisher downloads.

