# Narrow compatibility patch

Upstream: osmpbf 0.3.8, https://github.com/b-r-u/osmpbf, crates.io release.
MIT/Apache-2.0 notices retained. Only library/build sources are included; upstream
example, benchmark and external geographic test targets are omitted.

`DenseNodeInfoIter` previously required changeset, UID and user-name deltas before
returning a node version/timestamp. Geofabrik public files omit contributor fields.
Read those omitted contributor arrays with protobuf zero defaults; keep the required
version/timestamp checks. Relic reads only version/timestamp and never
interprets the default contributor values as real identities. Raw PBF remains the
authoritative original. Root fictional public-metadata PBF regressions cover this.

Replace this vendored patch with a released upstream fix after the same regressions
pass. `PrimitiveBlock::validate_snapshot` additionally checks parallel array lengths,
UTF-8/tag references and delta arithmetic before Relic invokes convenience iterators.
Partial metadata arrays are rejected, not treated as omitted. A dense block carrying
only one of version/timestamp is explicitly unsupported instead of silently losing
its metadata. Other consumers of the crate retain the upstream permissive iterator
API; Relic calls the strict preflight. Compressed blobs are bounded after decompression, require a matching raw_size, and are read through the zlib checksum before protobuf decoding.
