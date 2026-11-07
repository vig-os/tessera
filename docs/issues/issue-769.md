---
type: issue
state: open
created: 2026-11-06T14:42:13Z
updated: 2026-11-06T14:42:13Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/769
comments: 0
labels: feature, area:core, area:io
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-07T08:18:16.339Z
---

# [Issue 769]: [feat(io): strict-Vortex vector search — exact scan, IVF-flat in Vortex, optional HNSW as an ADR-0054 aux cache](https://github.com/vig-os/tessera/issues/769)

## Motivation

Similarity search ("find similar scan", semantic lookup over annotations/reports, dedup, agent retrieval) needs
embedding vectors stored next to the rows they describe, and a way to query them by nearest neighbour **with the
same filters, policy and provenance as every other tessera query**.

#299 lists an *external* vector DB (LanceDB/Qdrant/pgvector/Milvus) keyed by product id as one RDM-level
integration. This issue is the **in-format** counterpart, under one constraint: **strict Vortex**. Embeddings live
in a Vortex table block; any ANN structure is either more Vortex or a derived, rebuildable cache. No Lance, no
second columnar format, no vector DB as the source of truth.

## What already exists

- **Table blocks are a single deterministic Vortex file** (ADR-0024, `vortex` 0.75 fork at `608d8a1`), digested by
  blake3, with row groups (ADR-0026), chunk statistics (ADR-0059) and a bounded-memory DataFusion provider
  (ADR-0062).
- **The pinned fork has a native `fixed_size_list` array** (`vortex-array/src/arrays/fixed_size_list`), so a
  `[rows, dim]` embedding column can be stored as one Vortex column without expansion.
- **Derived caches have a rule already** (ADR-0054 Decision 2): unsealed, rebuildable, stamped with the
  `built_from` seal they mirror; freshness is a hash comparison. An ANN index is exactly such a cache.
- **Float quantization precedent:** #310 (int + scale, exact aggregation) shows how lossy-but-declared encodings
  are recorded.

## The gaps

1. **Ingest rejects wide fixed lists** (tracked in #768, which this issue depends on). `MAX_FIXED_LIST_WIDTH = 8`
   (`tessera/crates/tessera-ingest/src/arrow_table.rs:106-113`, ADR-0056 §2) expands `FixedSizeList<scalar, N ≤ 8>`
   into `N` columns and rejects wider lists, since a wide list is usually a flattened volume. A 384/768/1024-wide
   embedding is neither a tuple nor a volume. It needs a **declared vector column kind** that is stored natively
   as a Vortex `fixed_size_list`, with no expansion, and only when the schema says so (so the ADR-0056 guard keeps
   catching misused volumes).
2. **dtype.** `Float16` is widened to `F32` on ingest (`f16_widen`). Embeddings want `f32` (exact), `i8` + scale
   (≈4× smaller, small recall loss), and optionally `f16`. Note that pcodec array blocks (ADR-0023) exclude
   `int8`/`float16`, which is another reason the home for embeddings is a table block, not an array block.
3. **No distance kernel.** There is no `vec_distance(col, $q, metric)` (cosine / dot / L2) over the streaming
   provider, so even exact search needs custom code.
4. **No index lifecycle.** Nothing builds, stamps, invalidates or rebuilds an ANN structure.
5. **Policy inside the search.** A filter applied *after* top-k leaks or empties results. Pre-filters (access
   attributes, product/role/time) must apply before or during the neighbour search. Embeddings are as sensitive
   as their source text or images (ADR-0040), so they get the same confidentiality rules.

## Proposal: three tiers, all Vortex or derived from it

**Schema.** A vector field declares `{dim, dtype: f32|i8(+scale)|f16?, metric, model: {id, version, digest}}`.
Embeddings from different models are not comparable, so the model identity is part of the field and part of every
index key.

**Tier 0: exact scan (first, and probably enough to ~10⁶ rows per query scope).**
A DataFusion scalar UDF `vec_distance` over the ADR-0062 provider, with SIMD on the contiguous `fixed_size_list`
buffer, plus SQL pre-filtering and pushdown on the other columns:
`SELECT id FROM t WHERE <filters> ORDER BY vec_distance(emb, $q, 'cosine') LIMIT k`.
It's exact, needs no index, and works in `tessera-wasm`.

**Tier 1: IVF-flat built from Vortex itself (a strict-Vortex ANN).**
- A derived product holds a **centroids table** (Vortex, `k ≈ √n` rows) and the embedding table **re-emitted
  sorted/partitioned by `cluster_id`**, so each row group covers few clusters.
- Query: nearest `nprobe` centroids → `WHERE cluster_id IN (…) AND <filters>` → chunk statistics (ADR-0059) prune
  row groups → exact distance on the survivors.
- No foreign index format, and it reuses pushdown, streaming and provenance. k-means runs with a recorded seed,
  so the derived product is reproducible and can carry normal provenance.

**Tier 2: graph index (HNSW) as an ADR-0054 aux cache, only if latency demands it.**
`aux/<field>.<model>.hnsw`, stamped `built_from = <block digest>`, keyed by row id, filtered search through a
predicate callback, exact rerank from the Vortex column. Unsealed and rebuildable; a stale stamp falls back to
Tier 1/0. Library choice is an open question.

## Non-goals

- Lance or any second columnar format; a vector DB as the source of truth (#299 stays an *export* integration).
- Computing embeddings inside tessera. Producers supply them, with the model identity recorded in provenance.

## Open questions

1. Does `fixed_size_list` round-trip **deterministically** through the fork's writer (the ALP-exclusion lesson from
   ADR-0024)? Probe before committing to it.
2. `f16`: keep widening to `f32`, or support it natively for vector fields?
3. Tier 2 library: `usearch` (C++ core, fast, filtered search) vs a pure-Rust HNSW (wasm-friendly)?
4. Should Tier 1's partitioned emission *replace* the original row order, or live beside it (with row-id mapping)?
5. Collection scope: one index per product, or a collection-level derived index (ADR-0054 catalog altitude)?

Depends on: #768 (native `fixed_size_list` carrier for declared vector columns).

Refs: #299 (external vector DB integration), #310 (quantization precedent), ADR-0024, ADR-0026, ADR-0040, ADR-0054,
ADR-0056 §2, ADR-0059, ADR-0062.

