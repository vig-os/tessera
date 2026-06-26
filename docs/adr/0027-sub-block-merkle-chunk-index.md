# ADR-0027 — Sub-block Merkle + content-addressed chunk-index block (per-chunk confirmation + pruning)

Status: **Proposed** (2026-06-26) · Tracks `#214` · Builds on ADR-0020 (identity/Merkle), ADR-0024
(table payload), ADR-0026 (streaming compaction); realizes S2 (granularity) + S3 (Merkle-chunk index).

## Context
Today the Merkle is **one level deep**: `content_hash = merkle_root([block.digest])`, and a block's
internal chunks — array 64³ cubes, table 65536-row groups — are sub-block *layout*, folded inside the
block's single digest and invisible to the Merkle. For DAQ/streaming + range reads we want **per-chunk
confirmation**: verify one chunk/row-group on its own, and during capture advance a live integrity
signal per row-group that reconciles with the sealed identity.

## Decision
1. **Sub-block Merkle (design-2).** A chunked block's digest becomes the Merkle root over its
   per-chunk leaf digests: `block.digest = merkle_root([blake3(chunk_i)])`. Any chunk is then verifiable
   via a log-sized **inclusion proof** without rehashing the whole block. The **fixed grid** (64³ /
   65536 rows) makes the leaves a pure function of the data, so the *live* per-row-group root during
   capture and the *sealed* identity reconcile exactly.
2. **Leaves live in a dedicated Vortex *chunk-index* block — not the manifest, not the data table.**
   The index is tabular: `(block, chunk_idx, offset, n_rows, per-col min/max, blake3)`. Store it in
   Vortex (reuse the table substrate; **no third storage kind**). It is referenced in the manifest, and
   **its own digest is folded under the top-level Merkle** — so the index is itself tamper-protected.
   - NOT the manifest JSON: at scale this is MBs of hashes; the manifest must stay a small,
     range-readable spine you fetch before any block.
   - NOT a column on the data table: that is **circular** (the table's hash would depend on a column
     derived from the table's hash). It must be a *separate* block.
3. **Integrity ⊕ pruning in one structure.** The same index serves Merkle proofs **and**
   chunk-skipping / predicate pushdown (via the min/max stats) — the fusion of S3 (integrity index) and
   S2 (granularity/pruning). Uniform across array (64³) and table (row-group) chunks.
4. **Non-circular layering.** `chunk_leaf = blake3(chunk bytes)` (from the data) → `data_block.digest =
   merkle_root(leaves)` → the leaves are written into the index block → `index_block.digest =
   blake3(index bytes)` → `content_hash = merkle_root([data_block.digest, index_block.digest, …])`. The
   index depends on the data; the manifest depends on both. A verifier recomputes leaves from the data,
   checks them against the index, and checks the sub-root equals `data_block.digest`.
5. **Alternative — blake3-native (Bao).** blake3 is itself a tree hash, so any **byte range** of a block
   can be verified against the *existing* `blake3(block)` digest with a log-sized proof and **no format
   change**; the outboard hash-tree can live in the same index block. Lighter (free against the current
   digest) but confirms byte ranges at blake3's internal granularity, not semantic chunks, and gives no
   live per-row-group identity. Use it for "verify a range I range-read"; use design-2 for the streaming
   identity story.

## Durability vs the canonical bytes (the time/size-flush rule)
A low-rate acquisition may **flush partial fragments on a timer** (bounded durability latency), not only
when a row-group fills. This **rides the existing A/B/C commit path** (ADR-0026, `WriteSession`):
durable fragment → journal line (the watermark) → Merkle fold → `recover` replays to the watermark.
The flush trigger is the *only* change (timer ∨ buffer-full); the mechanism is unchanged.

**The caveat:** unlike today's *whole-block* streaming (where committed fragments **are** the final
blocks, packed as-is), sub-block/row-group streaming **compacts** at seal — fragments are durable
**staging**, and `seal` re-chunks them to the **fixed 65536 grid** via `encode_streaming`. Consequences:
- The sealed bytes are a pure function of *(rows, data)* — **independent of flush timing** → determinism
  + content-addressing preserved.
- The per-chunk header overhead of small time-fragments stays on **transient staging**, not the `.tsra`.
- A time-fragment is a **recovery unit, not a canonical Merkle leaf**: the design-2 leaves are the
  fixed-grid row-groups (recomputed at compaction). So there are two integrity notions — a
  fragment/watermark for *crash-recovery during capture*, and the canonical per-row-group Merkle that is
  the *sealed identity*. They coincide only when a row-group flushed exactly at the grid boundary.

## Consequences
- SPEC additions: sub-block Merkle leaf definition + the chunk-index block schema + a reserved name/flag
  marking it as integrity metadata (readers/schemas must not treat it as user data).
- `tessera-core`: sub-block Merkle (leaf + root + inclusion proof) helpers; `tessera-io`: emit the
  chunk-index block deterministically; reader verifies a single chunk via its proof; tamper-localization
  (which chunk failed, not just "block failed").
- Conformance: a multi-chunk fixture carrying a chunk-index block; the independent reader stays
  digest/Merkle-based (no codec decode), so it can verify proofs from SPEC alone.
- Backward-compat: additive. A product without a chunk-index block verifies exactly as today
  (block-level Merkle). The sub-block tree is opt-in per block.

## Status note
Proposed; sequence after the streaming **accumulator** (ADR-0026) so the row-group fragments it produces
become the leaves. Gated on the same cross-env determinism the rest of the format carries (#198).
