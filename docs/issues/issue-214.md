---
type: issue
state: closed
created: 2026-06-26T12:47:26Z
updated: 2026-09-28T17:54:12Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/214
comments: 2
labels: area:io
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-09-29T07:53:04.326Z
---

# [Issue 214]: [Sub-block Merkle + content-addressed chunk-index block (per-chunk confirmation + pruning)](https://github.com/vig-os/tessera/issues/214)

## Context
Today the Merkle is one level deep: `content_hash = merkle_root([block.digest])`, and a block's 64³ array cubes / 65536-row Vortex row-groups are sub-block *layout* — not individually hashed. For DAQ/streaming we want **per-chunk confirmation**: verify a single chunk/row-group (and, during capture, advance a live integrity root per row-group that reconciles with the sealed identity).

## Decision (design-2, see ADR-0027)
1. **Sub-block Merkle.** A chunked block's digest becomes the Merkle root over its per-chunk leaf digests (`block.digest = merkle_root([blake3(chunk_i)])`). Each chunk is then verifiable via a log-sized inclusion proof without rehashing the whole block. The fixed 65536 grid (and 64³ array grid) makes the leaves deterministic, so the **live** per-row-group root and the **sealed** identity reconcile exactly.
2. **Leaves live in a dedicated Vortex *chunk-index* block** — NOT the manifest JSON (megabytes of hashes at scale) and NOT a column on the data table (circular). The index is tabular: `(block, chunk_idx, offset, n_rows, per-col min/max, blake3)` → stored in Vortex (reuses the table substrate; no third storage kind). Referenced in the manifest; its own digest folded under the top-level Merkle (non-circular: index depends on data, manifest depends on both).
3. **Merges integrity + pruning (S2/S3).** The same index serves Merkle proofs AND chunk-skipping/predicate-pushdown via the min/max stats. Uniform across array (64³) and table (row-group) chunks.
4. **Alt: blake3-native (Bao).** Verify any byte *range* against the existing `blake3(block)` digest with no format change; needs an outboard hash-tree that can live in the same index block. Lighter (free against current digest) but not aligned to semantic chunks / live row-group integrity.
5. **Time-or-size flush is a durability knob, not a format property.** Low-rate acq may flush partial fragments on a timer for durability, but the sealed block always re-compacts to the fixed 65536 grid via `encode_streaming` → deterministic bytes regardless of flush timing; the per-chunk header overhead stays on transient staging, not the file.

## Done-when
- ADR-0027 accepted; sub-block Merkle leaf definition + chunk-index block schema in SPEC.
- `tessera-core`: sub-block Merkle helpers; `tessera-io`: emit the chunk-index block (digests + stats) deterministically; reader verifies a chunk via inclusion proof.
- Conformance: a multi-chunk fixture with a chunk-index block; tamper-localization test (which chunk).
- Relates to ADR-0026 (streaming compaction), S2 (granularity), S3 (Merkle-chunk index).
---

# [Comment #1]() by [gerchowl]()

_Posted on June 26, 2026 at 04:56 PM_

**Chunk-index CORE implemented** (`1499361`, `tessera_core::chunk_index`) — the structural foundation (ADR-0028 §3, absorbing this issue).

`ChunkStats` monoids (count/min/max/sum-i128, identity + associative combine) → one fold serves the index, the Merkle tree, and the pyramid. `ChunkIndex.root()` = MMR root over per-chunk digests (ties to the §1-2 integrity hierarchy). `prune(lo,hi)` skips non-overlapping chunks with **no false negatives** (conservative min/max). 8 property tests (monoid laws, roll-up homomorphism, exhaustive no-false-negative pruning, root==MMR, serde). Fresh-context reviewer APPROVED.

**Next:** wire per-chunk stats extraction into the table encoder (row-groups already split at `ROWS_PER_GROUP`) + array chunks, emit the chunk-index as a real block, and measure **#221-B** (leaf/chunk granularity → index overhead vs pruning selectivity) at that point.

---

# [Comment #2]() by [gerchowl]()

_Posted on September 28, 2026 at 05:54 PM_

Closing as **done** — verified on `origin/dev` in the 2026-09-28 backlog triage.

Evidence: ADR-0028 accepted (docs/adr/0028-unified-hierarchy.md status: Accepted, as-built). Chunk-index landed 1499361 + wired into table encoder cdeb1d6; block-digest vs sub-block-Merkle-root nuance resolved 7ea0ae9 (PR #283).

https://claude.ai/code/session_01XdERKMVDAwfMJSKdTytNnK

