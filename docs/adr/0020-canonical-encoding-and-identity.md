# ADR-0020 — Canonical encoding + identity model + manifest/BlockRef schema

**Status:** Accepted (P0) · **Register:** D4, D5 · **Issue:** #195

## Context
Tessera products are content-hashed and immutable. For that to mean anything across machines,
language implementations, and time, three things must be pinned: (1) how the manifest is serialized
when hashed (any byte difference changes the hash), (2) what "identity" is and how it differs from
"contents", and (3) the exact field set + invariants of the manifest and block references. The spike
hashed identity over an ad-hoc ``-joined string and rolled only block digests into
`content_hash` — so the manifest's own metadata (name, sources, schema) was **not** tamper-evident,
and the hash wasn't reproducible by a second implementation.

## Decision

### D4 — Canonical encoding = RFC 8785 JCS
All hashing is over **RFC 8785 JSON Canonicalization Scheme** bytes (`serde_jcs`): lexicographically
sorted object keys, no insignificant whitespace, ECMAScript-canonical number forms. JSON (not CBOR)
because the manifest is JSON-native, human-readable, and FAIR-legible; JCS is the established standard
for deterministic JSON and is trivially reproducible by any language (JS/Python/Go/Rust all have it).
CBOR was rejected: binary, less legible, no size win at manifest scale.

### D5 — Three distinct hashes
- **`id`** — *logical identity*. `blake3:` over `JCS({id_inputs})`, where `id_inputs` is a declared,
  ordered key→value map (default keys: `product`, `name`, `timestamp`). Stable across re-ingest of the
  *same logical product* and rename-safe (renaming a file or re-encoding the bytes does not change it).
  `id` is **not** the Merkle root — re-deriving an equivalent product keeps `id` even if storage bytes
  differ. The declared `id_inputs` are recorded in the manifest so identity is transparent + verifiable.
- **`content_hash`** — *data fingerprint*. Merkle root (`blake3` hash-of-hashes) over the **ordered**
  per-block digests. Changes iff payload bytes change. Integrity-only (random access is Vortex/zarrs'
  job, not this tree).
- **`manifest_hash`** — *the seal*. `blake3:` over `JCS(manifest)` with the `manifest_hash` field
  itself omitted. Covers the **entire** manifest — metadata, `id_inputs`, `sources`, and every
  `BlockRef` (which carries its block digest), so it transitively commits to all payloads too. This is
  the single root a signature / container index / WORM record binds to. Tamper any byte anywhere →
  `manifest_hash` changes. "Seal = hash of hashes" = `manifest_hash` over a manifest that embeds
  `content_hash`.

### Manifest & BlockRef schema (normative field set)
`Manifest` = `tessera_version` · `id` · `id_inputs` (ordered map) · `product` · `name` · `description`
· `timestamp` (RFC 3339, normalized to UTC) · `schema?` · `blocks: [BlockRef]` (order is significant —
fixes the Merkle order) · `sources: [Source]` · `content_hash?` · `manifest_hash?`. The two hash
fields are `None` while building, `Some` after `seal()`.
`BlockRef` = `name` · `kind` (`array`|`table`) · `digest` (required at seal) · `spec` (shape-specific,
opaque to the core). A missing digest at seal is a hard error (else a block would be invisible to the
content hash yet present in the manifest).

### Verification
`Manifest::verify()` recomputes and checks all three hashes: `id` from `id_inputs`, `content_hash`
from block digests, `manifest_hash` from the canonical bytes — returning typed `Integrity` errors that
name the field, expected, and actual values. Re-serializing a parsed manifest through JCS reproduces
`manifest_hash` byte-for-byte (the conformance "canonical encoding" gate).

## Consequences
- Hashing is reproducible across implementations and versions → unblocks fixtures, the conformance
  corpus, and the v1.0 independent reader.
- The whole manifest is now tamper-evident (closes the metadata/sources gap).
- Pre-1.0, so changing the `id`/seal derivation now (no persisted products yet) is free.
- `serde_jcs` is the one new core dependency (pure Rust; `itoa`/`ryu` for canonical numbers).

## Reconciliation — `content_hash` construction superseded by ADR-0028
The `content_hash` defined here is the **single-level / flat** root over the ordered block digests
(`blake3` of the concatenated digests — one level, no interior tree). **ADR-0028 supersedes this
construction** with a **recursive MMR** root (a node-hash applied at every level + an append-friendly
history tree). That is a deliberate **v0.2 identity revision** — `content_hash` values change and the
golden corpus regenerates with it. Scope of the supersession is **only the `content_hash` construction**:
this ADR's `id` (logical identity), `manifest_hash` (seal), JCS canonicalisation, BlockRef/Manifest
schema, and error taxonomy are **unaffected and remain Accepted**.

**As-built (2026-06-26):** the MMR root (ADR-0028 §1–2) is now implemented in `tessera_core::hash`
(domain-separated leaf `0x00` / node `0x01`, peaks-carry, bag-the-peaks; incremental == batch). The flat
construction is **superseded** — `content_hash` is the recursive MMR root, and the golden corpus, SPEC §3,
and the pure-Python reference reader were regenerated to it (conformance 3/3 + reference reader 6/6).
(ADR-0028 remains **Proposed** for its further parts — chunk-index, pyramid, sidecars, fused pass.)
