# Natural Earth local locator release

Accepted on 10 September 2026 for **small-scale local basemap display only**. These real physical land/coastline geometries are separate from fictional acceptance evidence. They do not define analytical geography, legal borders, facility identity or an incident's support.

The selected publisher repository release is [v5.1.2](https://github.com/nvkelso/natural-earth-vector/releases/tag/v5.1.2), published **13 May 2022**, pinned to commit `f1890d9f152c896d250a77557a5751a93d494776`. That date identifies the repository release, not a survey or observation date. No per-feature real-world dates are supplied. [manifest.json](manifest.json) records acceptance, exact asset paths, checksums, rights, identity and limitations; [discovery.json](discovery.json) records the actual GitHub API discovery and commit-pinned download URLs.

| Local asset | Use and actual check |
| --- | --- |
| [display/land.geojson](display/land.geojson) | 127 land polygons, 128 closed rings, 5,143 finite in-range coordinates |
| [display/coastline.geojson](display/coastline.geojson) | 134 coastline lines, 5,128 finite in-range coordinates |
| `raw/` | Exact two publisher GeoJSON files, `LICENSE.md` and `VERSION`; 278,067 bytes of geometry in total |
| [quality.json](quality.json) | Reproducible geometry, identity, byte-count and digest results; disclosed bounds-metadata and classification findings |

The frontend can copy both display files unchanged into its local public assets and render them with MapLibre fill/line layers. Its attribution should read **Made with Natural Earth · 1:110m · v5.1.2**, linking to [Natural Earth](https://www.naturalearthdata.com/). The publisher permits use and redistribution under its public-domain dedication; the pinned licence is retained, and the [current terms](https://www.naturalearthdata.com/about/terms-of-use/) were checked. Attribution is a deliberate provenance aid even though the publisher does not require it.

Display files preserve all raw coordinates and properties. Their IDs qualify the zero-based raw feature index by product and release because these publisher GeoJSON files do not supply persistent feature IDs. They are display identities, not cross-release physical identities. Top-level display bounding boxes are recomputed from actual coordinates; the coastline's publisher bbox has finer precision than its coordinates. One coastline feature retains `featurecla=Country` as publisher metadata; this does not grant it an analytical or political-boundary role.

Run from the Relic project root:

```sh
node validation/check_natural_earth.mjs
```

This dependency-free packaging check is offline and reads no private configuration. `--write-derived` rebuilds the two display files and quality report from the immutable raw inputs; it does not download data or accept another release. The application backend/importers remain Rust. Coordinate/ring checks establish structure, not surveyed accuracy or a complete topology audit.

This basemap acceptance applies only to the two physical products above. A separately recorded [place-navigation index](PLACES.md) now supplements them with country/territory labels and selected major/capital cities from the same pinned repository release; its dedicated manifest limits their use to camera navigation. The rest of the research catalogue remains disabled candidates. No inherited pack was promoted.

A later, separate [message-processing acceptance](PROCESSING.md) permits exact identity matching against all 243 original publisher city points, with city-level approximation and explicit occurrence gates. Its country-name candidates carry no occurrence geometry. It does not reuse the camera-navigation grant or promote inherited annotations.
