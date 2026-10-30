---
type: issue
state: open
created: 2026-10-30T02:06:59Z
updated: 2026-10-30T02:29:00Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/719
comments: 2
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-30T08:02:13.759Z
---

# [Issue 719]: [feat(format): schema-declared write-time aggregation views as derived blocks (exact, deterministic, parallel)](https://github.com/vig-os/tessera/issues/719)


Relates: ADR-0028 §3–§5, ADR-0059 (Proposed), ADR-0042, ADR-0054, #522, #347, #657, #297, #681.
Downstream consumer: MorePET/visionx-rs#18 (owner decision 2026-10-30).

## Principle (downstream owner, 2026-10-30)
Every stage of the pipeline has the same shape: **write once, read many; FAIR and agent-readable by construction.**
1. Scanner raw files are packed verbatim into a raw `.tsra`, together with provenance and a metadata sidecar.
2. Processing produces first-class `.tsra` products (Vortex tables, Zarr arrays), `derived_from` their input. Each
   product carries **views declared in its schema and computed during the write**: QC aggregates, pyramids, stats.
3. Every later stage (grouping, calibration, reconstruction, …) does the same, with its own view config.

So the ask is not "PET QC in tessera". It is that **any product can declare derived views that the write path
materialises**, so no reader ever re-scans the level below to learn what is in it.

## Use case: list-mode QC at ingest
Pipeline:
1. Raw acquisition files are first sealed verbatim in a "bag": a blob product, ADR-0038.
2. An **import** decodes them into typed table blocks in a product `derived_from` the bag.

The views belong to that import write, not to the bag. A list-mode scan decodes into coincidence rows (pairs):
up to about 2.4×10⁹ rows, from 19 GB of 8-byte packets. Each scan needs QC aggregates computed during the import
write, with no second pass:
- 1-D spectra: energy, 2 sides × 2 classes × 256 bins; TOF, 2 classes × 512 bins.
- 2-D: energy_pair, 2 × 256×256 (1 MiB of u64).
- 3-D or 4-D: energy_tof, 2 × 2 × 65 × 512, on non-uniform edges {0,4,…,252,255,256}, so the saturated code 255 has
  a bin of its own.
- Per-module counts: modules come from a versioned crystal→module map, and unmapped hits are counted explicitly.
- Time-bucketed spectra and counts, per second.

Every cell is an exact u64. Edges are fixed by the **domain**, not by the observed range, so cells are comparable
across scans. The output must be byte-identical whatever the thread count or chunk size. Ingest overhead must stay
under 5 % of end-to-end conversion time. Nothing here is PET-specific: it is "histogram of columns X, Y, split by
column C, keyed by lookup(K) or time bucket T".

## Requested capability (PROPOSAL)
1. **Declaration.** A product schema declares named views. Today `ProductSchema` has only `fields`, `blocks` and
   `requires_generation` (`tessera-core/src/schema.rs:228–242`); there is no view or pyramid declaration. Each view
   gives:
   - its axes: a column plus an explicit edge list, or `{lo, hi, step}`, always half-open with an exclusive last edge;
   - optional conditioning or split columns, small integer or boolean domains;
   - an optional **lookup-mapped key**: a column mapped through a caller-supplied table whose digest is recorded,
     with an explicit unmapped counter;
   - an optional **time-bucket key**: `floor(col / width)`, emitted as a sparse keyed table, not a dense time axis;
   - the cell type, `u64` only.
   The declaration and the lookup digest are sealed provenance, in the manner of `Generation` (ADR-0058 §2).
2. **Exact checked counts.** Overflow fails closed and is never wrapped, as ADR-0059 M2 requires. Note that today
   `ChunkStats::combine` uses unchecked `+` (`tessera-core/src/chunk_index.rs:146–164`), and only `checked_combine`
   (`:106`) fails closed.
3. **Determinism by algebra** (ADR-0059 §1). Cells are integers merged commutatively, so the output bytes do not
   depend on worker count, chunk size or merge order. The layout is fixed: axis order with the last axis fastest,
   row order, and row groups.
4. **Persistent per-worker accumulators.** Each worker keeps one dense accumulator across all batches and merges it
   once at end of file, never per chunk. Per-chunk dense partials cost 184 µs per chunk of serial merging in our
   probe, about 7.7 % of decode. The final merge must be **parallel**, for example split by bin range across the
   pool. Today ours is T−1 serial merges (see the cost model below).
5. **Hook placement.** Today the only write-time fold is `TableStreamWriter::with_live_index`. It handles one
   `stat_column` (`tessera-io/src/accumulate.rs:204–207, 237`) and folds it on the serial flush path once per row
   group (`:299–309`). Views cannot run there: per-event work on a serial path is exactly the cost we measured as
   prohibitive. Two shapes are acceptable:
   - (a) tessera exposes the view accumulator as a type the **producer's** decode workers drive while the columns
     are still hot in cache, and the writer merges, stamps and seals the results;
   - (b) tessera's encode pool (ADR-0028 §5) drives per-worker persistent accumulators.
   We prefer (a), because it avoids a second traversal of cache-cold columns.
6. **Bounded memory and admission.** A preflight computes slots × cells × 8 B, plus the merge destination, before
   any data flows. Configurations over budget are rejected, or the worker count is reduced, never discovered as an
   out-of-memory error. Sparse time-keyed output streams out in bounded batches.
7. **Storage as derived blocks inside the same product, excluded from canonical identity** (ADR-0028 §4). This
   requires a seal fix (next section). Each view block records `class: "derived"`, a `recipe`, and the digests of
   the blocks it was built from, the same idea as ADR-0059 §7 M1 `indexed_digest`.
8. **Read side.** A view reads back as an Arrow table or array, with self-describing edges, units and the
   availability state. `tessera verify` can recompute any view and compare it ("verifiable", ADR-0028 §4).

9. **Views are mergeable aggregate queries, materialised as a pyramid of partials (chunks of chunks).**
   - **The declaration is a restricted aggregate query**, e.g. DataFusion-style
     `COUNT(*) GROUP BY bin(energy_a, E), bin(energy_b, E), class`, limited to **monoids**: count, sum, min/max and
     fixed-edge histograms, whose partials combine exactly in any order. Non-mergeable aggregates (median, distinct)
     are out of scope, or need explicit sketches.
   - **Partials sit at chosen levels of the chunk tree.** Each chunk yields a partial (this generalises #522's
     histogram monoid from one column to declared N-D views). Partials then merge up a tree: chunk → block or time
     bucket → product → Collection. That gives:
     - (i) answers at any granularity with no decode, since a time range is a subset of partials;
     - (ii) a parallel tree merge instead of a serial T−1 chain;
     - (iii) cohort roll-ups by merging product-level partials.
   - **Size decides the levels.** A 1 MiB energy_pair partial per 64K-row chunk would be larger than the data it
     summarises. So each view declares the levels at which it is materialised, e.g. 1-D spectra per chunk, large
     2-D/4-D views only per time bucket or per product, and the preflight admits the total.
   - **Pyramids are the same mechanism**: an array view whose monoid is downsampling. Declaring both once, as
     `(monoid, axes/keys, levels)`, would subsume `tessera pyramid`.

## Blocking defect: derived blocks are folded into identity today
- `ProductBuilder::seal` collects **every** block digest into `content_hash = merkle_root(&digests)`
  (`tessera-core/src/product.rs:172–182`). `Manifest::recompute_content_hash` does the same
  (`manifest.rs:159–168`).
- `manifest_hash` covers the canonical bytes of the whole manifest, including every block ref (`manifest.rs:146,
  171–173`).
- `chunk_index_block` tags `.cidx` as `class: "derived"` (`tessera-io/src/chunk_index.rs:39–48`), but its own test
  asserts that the block still "rolled into the content hash" (`:88–100`).
- The ADR-0028 header reads "Accepted (as-built)" (line 3), but its status note says "Proposed" (199–203). §4's
  identity exclusion is **not implemented anywhere** that we could find.

Proposed resolution (PROPOSAL; owner decision for tessera):
- `content_hash` folds only `canonical` blocks;
- derived refs sit in a manifest tier covered by `manifest_hash`, so views stay sealed and signable;
- adding or regenerating a view becomes an ADR-0036 `evolve`: same `id`, same `content_hash`, new `manifest_hash`.

The alternative is ADR-0042 `aux/`, which is excluded from both hashes but unsealed and unsigned. That is too weak
for QC evidence. No corpus product carries a `.cidx` (ADR-0059 "Why this is one event"), so the fix is cheapest if
it lands **inside the ADR-0059 format event**.

## How this relates to existing work
- **ADR-0059 / #522.** These are complementary, not duplicates. ADR-0059 builds one block-level, unconditioned
  1-D histogram per block: observed-range edges, with a second pass after min/max (§5), MAX_BINS 4096. It
  explicitly **rejects dtype-domain edges** (§5, lines 174–178). Views need caller-fixed domain edges (so no
  prepass), N-D cells, conditioning and keys. Both share the integer-only rule (§1), fail-closed overflow (M2) and
  source binding (M1). The proposal is that ADR-0059's histogram becomes one built-in view kind.
- **Pyramids.** Not schema-declared and not built at write time. `tessera pyramid` (`tessera-cli/src/nav.rs:1304–1366`)
  decodes a sealed array after the fact and writes a **separate product** `derived_from` the source, with level
  blocks that carry no `class: "derived"` tag. That contradicts ADR-0028's "derived sidecar blocks in the same
  product … never separate products" (lines 181–182). Views would be the first declared, write-time derived
  blocks; pyramids could move onto the same declaration later, as views of `(monoid, axes, depth)`.
- **#657** (a recipe-only view product, read-time SQL) and **#297** (histograms in tessera-explore, read-time, two
  passes) are read-time views. This proposal is the write-time, materialised end of the same idea. A materialised
  view could cite a #657 recipe id.

## What stays domain-side (visionx)
- The view *configuration*: which views, the edges, the conditioning columns, and the module map with its digest.
- Decoder-specific temporal tables: segment normalisation across clock resets and gaps, joining split seconds, and
  bed-position carry-forward. These are emitted as ordinary table blocks (or derived blocks).
- The decoder ledger and count reconciliation (pair rows = Σ every view; marginals equal bin-for-bin).
- Flag and threshold evaluation, done at read time from the view integers.

## Performance requirements: measured cost model (visionx, synthetic, warm cache)
These are our numbers for the same job done in visionx. They set the bar that a tessera implementation must beat.
- Workload: paired interleaved harness, main c902b93, 8 exclusive vCPUs, ksb, 90 rotated rounds, 524,288 rows per
  decode.
- Decode alone costs 63.0 / 31.3 / 24.0 ns per event at 1 / 4 / 8 threads.
- **Added observe cost is about 24 ns per event of wall time, flat across threads** (25.4 / 23.7 / 23.9 ns).
- As overhead relative to decode alone:

| Threads | Observe overhead | Checked overhead (observe + temporal + accounting) |
| ---: | ---: | ---: |
| 1 | 43 % | 79 % |
| 4 | 68 % | 88 % |
| 8 | 100 % | 118 % |

- Dense payload is **2,129,952 bytes per worker slot**: 266,244 u64 cells. At 8 threads that is 16.25 MiB of
  slots, against a 16 MiB last-level cache.
- **T−1 serial merges at end of file**: 266,248 counters each, so 1,863,736 checked additions at 8 threads. A local
  2-thread merge took 2.356 ms, 7.25 % of wall time.
- Target: **< 5 % of end-to-end conversion** (decode + encode + write + fsync). That denominator does not exist yet
  (writer bounds: #681). Against decode alone at 8 threads, 5 % is about 1.2 ns per event. Even an end-to-end
  denominator two to three times larger requires roughly a **5–10× reduction** in added wall time.
- Sources: visionx-rs#18 (comment 6512490807), visionx-rs#125 (comment 6512642497).

## Optimisation expectations (PROPOSAL; to be measured, not assumed)
- **Batched, branch-free index computation.** Compute the bin index for a whole batch first (SIMD lookup tables for
  small integer domains; one gather per edge set), then scatter.
- **Per-batch narrow counters.** Use u16 or u32 cells per batch: u32 cannot overflow within a batch of ≤ 2³² rows.
  Fold them into checked u64 at batch end. The working set halves or quarters, and the u64 overflow checks become
  per fold, not per event.
- **Lazy zeroing and touched-range tracking** of narrow counters, with dense fallback chosen by preflight (not at
  run time).
- **Parallel merge by bin range** across the pool. No serial T−1 chain and no nested pools.
- **Cache-aware layout** for the large 2-D and 4-D views, such as tiling, or splitting by class so each worker
  touches one half.

## Open questions for tessera
1. Should identity exclusion cover `content_hash` only (proposed above) or the seal too? Which ADR owns the change:
   an amendment to ADR-0028 §4 or ADR-0059's event?
2. Is the restricted-aggregate (monoid) declaration plus per-view materialisation levels (item 9) the right model?
   Should it be expressed as a DataFusion logical plan, or as a closed view-kind enum?
3. Where do declarations live: in `ProductSchema` (static), in `Generation.config` (per run, e.g. the lookup
   digest), or split between them?
4. Is a producer-driven accumulator (§5a) acceptable, given that `verify` recomputes from canonical blocks?
5. Can views be declared on a table written through `WriteSession` / the stream jobs (`stream.rs:147–183`, the
   `*_job` / `*_job_indexed` constructors), and do they ride the #681 bounded-buffer work?
6. Should pyramids migrate onto the same declaration, and should `tessera pyramid` stop emitting a separate product?
7. Is a first-class import from a bag in scope? visionx adopts the bag (stage 0) in its v0.1. Today `derived_from` products re-read the original files: no
   `FormatOptions` variant takes a `.tsra` blob as input (`tessera-ingest/src/spec.rs:148–283`). The edge is only
   pinned provenance (`engine.rs:307–327`). Should there be an import that reads the bag's blob blocks, streamed
   and digest-checked, so the views' `from` chain reaches the verbatim bytes?

---

# [Comment #1]() by [gerchowl]()

_Posted on October 30, 2026 at 02:09 AM_

Downstream note (visionx-rs architect), after discussing it with the tessera architect:
- **Declaration model (Q2):** visionx agrees with a **closed, versioned view-kind enum** in the schema, not a serialized DataFusion plan. DataFusion stays on the read side.
- **Kinds visionx needs:** `count`; `histogram` (fixed, possibly non-uniform edges, N-D, conditioning/split columns, an optional lookup-mapped key with a recorded digest and an explicit unmapped counter, an optional time-bucket key); `sum` (checked i64, e.g. signed TOF per second).
- **Ordering:** the identity fix (#720) has to land before any derived view.

---

# [Comment #2]() by [gerchowl]()

_Posted on October 30, 2026 at 02:29 AM_

**Updated cost model** (visionx-rs#125, comment 6513462032). Paired harness, 64× input-size sweep (6.3–403 MB), ksb, 8 exclusive vCPUs, 288 paired records.

**Added QC observe cost per event** (the slope across input sizes): **40.9 / 11.6 / 5.6 ns/event at 1 / 4 / 8 threads**. It scales about 7.3× with threads.

**Per-file fixed cost:** about **10 ms at 8 threads**, of which about 8.3 ms is the serial T−1 end-of-file merge. It stays the same from 6.3 MB to 403 MB.

The earlier "~24 ns/event, flat across threads" figure was this fixed cost spread over a small (6.3 MB) benchmark input.

**Implication for the views design:**
- On real scans (about 10⁹ rows), the per-event index and increment path dominates. That covers batched index computation, narrow per-batch counters and cache layout.
- A parallel or tree merge matters mainly for small inputs. It is still worth having, because chunk-level partials make merges frequent.
- `perf_event_open` is denied inside ksb pods (seccomp), so variant timing is the measurement method there.

