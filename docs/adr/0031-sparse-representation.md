# ADR-0031 — Sparse data representation: COO table convention (scatter), chunk-prune (block)

Status: **Proposed** (2026-06-26) · Tracks `#218` · Relates to ADR-0028 (chunk grid / stats / pruning),
ADR-0027 (chunk-index stats), ADR-0029 §6 (substrate-by-nature — density flip), ADR-0023 (array payload),
ADR-0024 (table payload), and the #213 codec-`auto` "decide once, store the concrete encoding" rule.

## Context
ADR-0029 §6 sends **mostly-empty grids** (sparse high-D histograms, sparse masks/labels, sparse matrices,
scatter point-clouds) to the **table** substrate as a COO `(coords…, value)` list rather than a dense
array. This ADR pins **how**, and **why not a bespoke sparse array codec**.

The framing matters: **Tessera is a storage/interchange format, not a compute engine.** Its measurable
surface is (1) **bytes on disk**, (2) the **I/O path** to a working set (bytes-read, RAM ceiling,
random-access, pushdown), and (3) the **zero-copy handoff** to Arrow/numpy. In-memory sparse *math*
(spmv, matmul, solves) is the downstream library's (scipy/numpy) concern — out of scope here, and not a
thing we benchmark (it would measure scipy, not Tessera).

## Decision

### 1. Two regimes, by *structure* not just occupancy
- **Block / clustered sparsity** (zeros in contiguous regions — a tumour mask, a cropped FOV) → **dense
  chunked array** (ADR-0023). Already handled *for free*: an all-zero 64³ chunk prunes by its `count=0`
  stat (ADR-0028) and pcodec crushes the zero-runs to ~nothing. **No COO, no new path.**
- **Scatter sparsity** (nonzeros spread thin enough that few/no whole chunks are empty — sparse high-D
  histograms, scatter point sets) → **COO table** (§2). This is the only regime that needs a convention.

So the lever is *occupancy × clustering*, not occupancy alone; the crossover is **measured** (§5), not
asserted.

### 2. Scatter form = a COO table (no new primitive)
A scatter-sparse grid is a **Vortex table block** with columns:
```
i0, i1, …, i_{n-1},  value[ , value1, value2, … ]
```
plus, on the block's `ArraySpec`-style metadata: the **dense `shape`**, **`dtype`**, and **`fill_value`**
(the implied value at absent cells — usually 0, but explicit so "absent" is unambiguous). A reader can
densify on demand from `(shape, fill_value, rows)`. Multi-component cells (a `[3,z,y,x]` deformation
field, complex values) are **extra value columns** — COO generalises without a new container. This rides
the existing Vortex encode/decode, **Merkle, monoid-stats, and predicate-pushdown** stacks unchanged.

### 3. Why not a bespoke sparse array codec (CSR/CSF)
A sparse *codec* inside the array substrate could be marginally smaller for *structured* sparsity, but it
would need a **parallel** integrity path, stats/monoid path, and chunk-pruning path — duplicating
ADR-0027/0028 for one data shape. COO-as-table is **composition over a new primitive** (ADR-0029 §1): the
hash tree, the `count`/min/max/sum monoids, range-pushdown on the coordinate columns, and the streaming
MMR append all **already work**. We adopt a sparse codec only if §5 shows a *large* on-disk gap at the
occupancies we actually encounter — the bench can override this default, not create it.

### 4. Stats, ordering, determinism
- **Monoids** (ADR-0028) fall out: `count` over the value column **is** the nnz; `min/max/sum/mean` over
  `value` are the nonzero stats; `count` over the *implied dense* grid is `nnz` + `(∏shape − nnz)`
  fill-cells (cheap — needs only `shape`).
- **Canonical row order** = lexicographic by coordinate tuple `(i0, i1, …)`. This makes the encoding
  **deterministic** (stable digest) *and* gives coordinate-range pushdown locality for free (a reader
  asking `i0 ∈ [a,b]` hits a contiguous run). Required, not optional.
- `fill_value` is part of identity (it changes what the array *means*), so it lives in the manifest under
  `manifest_hash`.

### 5. Threshold = **measured**, stored **explicit**, never inferred at read
Which regime an ingest picks (dense-chunked vs COO) is **explicit metadata on the block**, chosen at
write time and **recorded as the concrete encoding** — never re-decided or labelled "auto" at rest
(mirrors the codec-`auto` rule of #213: decide once, store the concrete answer, keep reads deterministic).
The decision boundary is set by a **lean crossover bench** (the empirical input, run with the 0026–0031
overhead spike):

> **Bench (decision-driving):** `dense+pcodec` vs `COO+Vortex`, swept over **occupancy** (≈0.01 %→50 %)
> × **structure** (random scatter · banded · block-clustered), measuring **on-disk size**, **read-RAM /
> bytes-read** for a region, and **materialize time**. The occupancy×clustering crossover *is* the
> default threshold.
>
> **Baselines (credibility, one row each — like #143):** `scipy.sparse.save_npz` (standard sparse on
> disk) and `numpy.memmap` (naive dense, paged). **Not benched:** spmv / matmul / in-memory sparse ops —
> downstream library layer, not the format's.

The threshold ships as a documented default an ingest *may* override per-array (the encoding is explicit
regardless), not a hard rule baked into the engine.

### 6. Materialize, don't densify (the handoff contract)
Tessera delivers the **coordinate + value columns** zero-copy to Arrow → a `scipy.sparse.coo_matrix`
(or CSR via one downstream conversion) is near-zero-copy. Producing a **dense** numpy array is an
**O(∏shape) scatter the consumer opts into**, never forced by the format. A `decode_subset`-style read
returns only the nonzeros within the region (read-RAM ∝ nnz-in-region, not region volume). The format's
obligation ends at cheap delivery of the working set; densification is downstream and optional.

## Consequences
- **No new primitive / no new substrate** — COO is a Vortex table with `shape/dtype/fill_value` metadata;
  block-sparse is the existing dense-chunked array + stat-pruning. Pure convention.
- **DRY integrity & query** — Merkle, monoid-stats, pushdown, streaming append all apply to the COO table
  unchanged; nothing is forked for sparsity.
- **Right-sized benching** — we measure only what the format owns (disk, I/O-RAM, materialize), not
  downstream sparse compute; the crossover bench sets one threshold and earns its place.
- **Deterministic** — canonical lexicographic row order + explicit `fill_value` → stable digest; the
  encoding choice is stored, not inferred, so reads never re-decide.
- **SPEC additions:** the block-vs-scatter regime rule; the COO column layout + `shape/dtype/fill_value`
  metadata; canonical row ordering; the monoid mapping; the materialize-don't-densify handoff contract.
- **Schema touch-points:** sparse histograms (ADR-0029 §6) and sparse masks/label-maps (ADR-0029 §4 ROI)
  declare the COO form; `fill_value` joins the array-spec field model.

## Status note
Proposed; decision (COO-table for scatter, chunk-prune for block) is settled in shape — the §5 **threshold
value** is the one empirical input, produced by the crossover bench in the 0026–0031 overhead spike that
promotes this set to **Accepted** (at which point the sparse convention must be cited in FEATURE-MATRIX
per the matrix gate).
