# Sources and adapters

Relic separates **what a source says** from **how it is fetched**. The core
owns scheduling, storage, catch-up safety, publication and the HTTP API.
Provider clients plug in through three small traits in
[`app/src/sources.rs`](../app/src/sources.rs) and are registered once at
startup:

```rust
relic_app::server::run(Sources {
    timeline: Some(Arc::new(MyTimeline::new(&settings))),   // a profile/post timeline
    aircraft: Some(Arc::new(MyAircraft::new(&settings))),   // a live aircraft layer
    ship_snapshots: Some(Arc::new(MyShips::new(&settings))), // a live ship-position layer
    timeline_setup: None,
}).await
```

The public binary (`app/src/main.rs`) registers none. Every source that is not
registered still appears in the interface, labelled **"Not included in this
build"**, rather than looking empty or broken.

## Built in

| Source | What Relic does with it |
| --- | --- |
| **Telegram** | Streams channels you add, with a recent-history baseline, exact catch-up after downtime and retained media. Edits keep the earlier revision. |
| **KeelWatch** (a private dataset platform for ship positions, not part of this repository) | Ship positions as vector tiles, past moments, ship tracks and the "ships near this message" finder. |
| **Reference geography** | GeoNames, Natural Earth, selected country/province boundaries, marine regions and Geofabrik OpenStreetMap extracts, imported explicitly per region. |

## The interfaces

### `PostTimeline`: profiles that publish posts

An adapter reads one page of a profile's timeline and returns each post
already normalized into Relic's message shape, with its exact numeric ID,
author, and whether it is an ordinary entry (pins and embedded posts are not).

The collector, not the adapter, decides what is safe:

- **Watermarks.** A source's checkpoint only advances after a page is durably
  stored, and only to an ID that was actually seen.
- **Gaps are stated, never filled.** If the previous checkpoint disappears
  before older posts appear, new posts are kept and the interval is marked
  incomplete for review. Crossing an ID boundary is not proof of completeness.
- **Repeated cursors** stop the run before another request and retry later
  from the same watermark with backoff.
- **Other authors** on a page (quotes, threads) stay in the raw page but are not
  published under the monitored source.

Failures are classified (`RateLimited`, `Unauthorized`, `Transient`,
`Unavailable`, `Other`) so the collector can apply cooldowns, ask for a
reconnect or retry, without knowing anything about the provider.

### `AircraftLayer` and `ShipSnapshotLayer`: live context

These return the JSON bodies the map draws, plus a health card. They are live
context, never evidence: nothing they return is published as a message.
Adapters are expected to cache briefly, label stale answers as stale and say
why a layer is degraded instead of returning an empty list.

## Adapters kept private

The adapters for **X**, **Flightradar24** and **MarineTraffic** live in a
separate private repository. They implement the traits above. That
repository builds the full `relic` binary against this one, and `./relic`
uses it automatically when it is checked out in `private-sources/`.

What they demonstrate, without their protocol details:

- Async Rust clients for undocumented mobile and browser protocols, including
  a hand-written protobuf/gRPC-web decoder with strict, bounded parsing.
- Every request goes through an explicitly configured proxy. A proxy failure is
  a source failure and is never retried directly. Response peers are checked
  against the proxy, and bodies are bounded while streamed.
- Proxy pools with per-proxy rest periods: a rate limit rests every proxy, while
  a transport fault rests only the proxy that carried it.
- 75 offline connector tests (captured request byte parity, hostile and
  malformed responses, cancellation of a pending proxy tunnel) plus 22 adapter
  and integration tests that run the real adapters inside this core against
  loopback proxies that never forward.

## Writing an adapter

1. Create a crate that depends on `relic-app` (this repository's `app/`).
2. Implement one or more traits from `relic_app::sources`.
3. Write a `main` that builds a `Sources` value and calls
   `relic_app::server::run(sources)`.

The core's tests use a recording probe that implements `PostTimeline`
([`app/tests/connections.rs`](../app/tests/connections.rs)). It is the smallest
working example.
