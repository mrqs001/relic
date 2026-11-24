# Relic reference data

Start with [DATA_PLAN.md](DATA_PLAN.md) for the selected datasets, alternatives, import order and quality rules. [catalogue.json](catalogue.json) is the machine-readable source registry: publisher links, access locations, identity fields, coverage, update and licensing notes for 51 candidate datasets across ten domains. A recommendation is not a claim that the data was downloaded or accepted into Relic.

For the selected regional OSM source, read the [Geofabrik PBF acquisition contract](../docs/REFERENCE_DATA.md#regional-geofabrik-pbf-acquisition). It covers discovery URLs, preservation and geometry rules, overlap handling, refresh strategy and implementation checks.

| Location | Purpose |
| --- | --- |
| `DATA_PLAN.md` | Dataset decisions and the path from acquisition to reviewed publication |
| `catalogue.json` | Candidate source contracts and evidence for selection |
| `accepted/natural-earth/` | Natural Earth (public domain): accepted country and place context |
| `candidates/` | geoBoundaries gbOpen country and province editions used by the boundary tests |
| `marine-selections.json`, `marine-feature-types.json` | Reviewed SeaVoX marine areas, their aliases and feature types |
| `geography-corrections.json`, `reconciliations/` | Reviewed name and boundary decisions, kept separate from publisher data |

Global reference files are acquired by the application under explicit area, version and resource bounds rather than bundled here. Publisher snapshots and attribution are kept separate from normalized records and display geometry. Refreshing a reference layer never overwrites analyst judgments or source history.
