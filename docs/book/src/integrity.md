# Integrity & verification

Tessera is designed to be **tamper-evident and offline-verifiable forever** — you can confirm a product
is exactly what was sealed without any network, key server, or original tool.

## What `verify` checks

`tessera verify` (shown in [Getting started](./getting-started.md)) is the whole chain: the mimetype
magic, the `manifest_hash` seal over the canonical (RFC 8785 JCS) manifest, and **every block digest**
re-hashed against the value the manifest records. Any changed byte — in a block or in the manifest —
fails. Verification is bounded-memory: a multi-gigabyte blob streams through the hasher at a few MiB of
RSS, so integrity doesn't cost you the payload in RAM.

## The Merkle structure

`content_hash` is a recursive Merkle Mountain Range over the ordered block digests (ADR-0028), which buys
two proofs beyond a flat hash:

- **Inclusion proof** — confirm one block or chunk belongs under `content_hash` without re-reading the
  rest (an audit path).
- **Consistency proof** — prove one revision's `content_hash` is an append-only *prefix* of a later one
  (the [versioning](./versioning.md) guarantee: a later version provably only *added*).

Per-chunk `{hash, stats}` leaves (the `ChunkIndex` companion) give the same at sub-block granularity,
and double as a pruning zone-map — "pruning never lies" is a gated invariant (a chunk that *could* match
a range is never skipped).

## The correctness gates

These are binary release gates (`FEATURE-MATRIX.md` §C), re-run every build:

- **Bit-exact lossless** — arrays and tables round-trip byte-identically, including float NaN / ±inf /
  −0.0 / denormals and integer limits (the clinical gate).
- **Writer determinism** — the same input produces byte-identical output, so `content_hash` is a stable
  identity, not a build artifact.
- **Pruning never lies** — proven exhaustively.

*Evidence:* `tessera_core::hash::{inclusion_proof,consistency_proof,…}`, `chunk_index::tests`, and the
`array`/`table` bit-exact + determinism suites. See [Conformance & the SPEC](./conformance.md) for the
cross-implementation gate.
