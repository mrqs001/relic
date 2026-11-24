# Approximate city references for message reading

[processing-manifest.json](processing-manifest.json) records a separate, narrow acceptance on 11 September 2026. It accepts the **243 original populated-place points** in the retained Natural Earth v5.1.2 publisher release for approximate city identity, plus **210 country/territory names** for broad contextual identity only. The existing basemap and camera-navigation grants remain unchanged. No inherited collection is used.

The city source is the unchanged `raw/ne_110m_populated_places.geojson`, SHA-256 `a86028b083182b68c7620fc6e1a8a47ee547cb9cd2fb62ccbb78bea786440899`, 654,324 bytes. The retained publisher repository commit is `f1890d9f152c896d250a77557a5751a93d494776`, released 13 May 2022. Each result retains the release, exact source hash, publisher `NE_ID`, normalized record hash, acceptance hash and precision. An individual city's observation date is unavailable; this is not a current survey.

`GeoResolver` matches literal source mentions to exact publisher names, multilingual publisher names or the three explicitly reviewed Tel Aviv spelling rules in [processing-aliases.json](processing-aliases.json). It normalizes case and whitespace only. Its country constraint uses each city record's own `ISO_A2`/`ADM0NAME`; it does not crosswalk through sovereignty labels. Alias hashes and their original publisher fields are checked at load time.

A single unambiguous city match may return an occurrence-location hypothesis only when the caller has classified the message as a **reported event** and the place has the **event** role. The supporting quote must appear exactly once and contain the literal mention. Its unchanged publisher city point is labeled `city_level_approximate`, never an observed incident-site coordinate. Origin, destination, issuer and other mentions remain context. Denied, planned, historical, promotional and analytical material receive no occurrence point from this resolver.

Countries remain broad name identities with no point or polygon in their candidates. An ambiguous name preserves alternatives; a country mismatch, unknown place or canonical-name-only suggestion preserves an unresolved result. Supplying a model coordinate is invalid. A corrupt or changed accepted reference fails loading instead of silently substituting a different source.

## Coverage and next acquisition boundary

This is a sparse worldwide city reference. Tehran, Beirut, Jerusalem, Tel Aviv, Damascus, Baghdad and Riyadh are present. **Haifa, Gaza, Isfahan, Aleppo, Basra and Mosul are absent**, including among the additional 43 points omitted from the camera-navigation index. A missing place stays unresolved; it does not move to the nearest known city or a national capital.

The next proposed coverage improvement is a separately accepted, scoped GeoNames release for Israel, Palestinian territories, Lebanon, Syria, Iran, Iraq and Jordan, with retained alternate names. Before any download/publication, apply [DATA_PLAN.md](../../DATA_PLAN.md): discover the current publisher release and rights, retain exact archives/hashes, validate country/feature classes and stable IDs, inspect multilingual ambiguities and populated-place precision, and publish a distinct accepted edition. No GeoNames acquisition or acceptance occurred in this change.

## Verification

Run `cargo test --manifest-path app/Cargo.toml processing_geo --lib` from the project root. Six tests cover the 24 fictional cases, original reference counts/point identity, known coverage gaps, injected coordinates, input/output bounds, source/alias checksum failures, alias publisher-field consistency and repeated quotes. The fixture covers English, Hebrew, Persian and Arabic; role separation; negation; planned/historical events; copied reports; source instructions; and same-name places in different countries.

[examples/processing-evaluation.json](../../../examples/processing-evaluation.json) has 15 development and nine held-out cases. Three cases explicitly target only the resolver: their proposed locations are deliberately wrong outputs used to test deterministic abstention. The other 21 are eligible for semantic model evaluation (13 development and eight holdout). These checks establish resolver behavior; they do not establish model extraction accuracy, calibration, independent corroboration or production readiness. The fixture messages are invented and must remain separate from real evidence.
