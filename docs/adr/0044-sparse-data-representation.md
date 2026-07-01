# ADR-0044 — Sparse data representation: COO table convention, no bespoke sparse codec

Status: **Proposed** (2026-07-01) · Tracks `#218` · Ratifies + pins the decision omnibus-recorded in
ADR-0031 (as-built) · Relates to ADR-0023 (array payload), ADR-0024 (table payload), ADR-0027 (chunk
stats), ADR-0028 (unified hierarchy — chunk `count=0` prune), ADR-0029 §6 (substrate-by-nature),
ADR-0032 (`fill_value`/`unit` on ArraySpec), and #262 (SQL over tables).

## Context

Tessera's substrate rule (ADR-0029 §6) sends **mostly-empty grids** to the **table** substrate as a
COO list of `(coords…, value)` rows rather than a dense array. #218 asks *how* — and specifically
whether a bespoke **sparse array codec** (CSR/CSF/CSF3) is ever worth the extra machinery.

Two shapes of sparsity show up in imaging:

- **Block / clustered sparsity** — empty regions are contiguous (a tumour mask, a cropped FOV, a
  reconstruction outside the body). The dense representation is already storable *and* efficient
  because empty chunks prune by `count=0` stats (ADR-0028) and pcodec crushes zero-runs.
- **Scatter sparsity** — non-zeros are spread thin (a sparse high-D histogram, a scatter point set,
  a sparse mask over a very large ambient grid). Whole chunks are rarely empty, so chunk-pruning
  doesn't help; the *ambient grid may not even be materializable* (high-D histograms).

PET listmode is *already* a table (rows of `(t, x, y, z, e)`), so it never enters this decision.
The decision is about **sparse volumes / masks / high-D grids**.

## Decision

### 1. Regime split — structure first, occupancy second

- **Block-sparse → dense-chunked array (ADR-0023) + `count=0` chunk-prune.** No COO, no new path.
  Empty chunks skip decode entirely (ADR-0028 §3). This is *already free*.
- **Scatter-sparse → COO table (ADR-0024) with array-spec metadata.** The canonical form when the
  ambient grid is either unstorable (high-D histograms) or when access is selective point-reads
  where nnz-bounded RAM/latency matters (the measured wins — see §4).

The lever is **materializability + access pattern**, not "% occupancy". The old occupancy heuristic
was tested (bench #221-A) and rejected: dense+pcodec beats COO on disk at every occupancy from
0.01 %→50 % at 128³ int16, because pcodec's zero-run compression + small-domain coordinate coding
means neither a 3-column nor a single-linear-index COO ever wins on bytes at materializable scales.

### 2. Scatter form — a COO table (no new primitive)

A scatter-sparse block is a **Vortex table block** (ADR-0024). Canonical layout:

```
(coord…, value[, value1, value2, …])
```

with the block's `ArraySpec`-style metadata carrying the **dense `shape`**, **`dtype`**, and
**`fill_value`** (the implied value at absent cells — usually `0` but always explicit, so "absent"
is unambiguous and part of identity).

Two coordinate encodings, one convention:

- **Multi-axis** — one column per axis: `(i0, i1, …, i_{n-1}, value)`. Range-pushdown on individual
  axes for free (columnar). The §2 spec form.
- **Single linear-index** — one C-order `u64` column + value: `(idx, value)`. More compact when
  rank is high or coordinates are wide. **This is the current `tessera_io::array::to_coo`
  emission** (ADR-0031 status note); the multi-axis form is a documented extension of the *same*
  convention. Both densify from `(shape, fill_value, rows)` identically.

Multi-component cells (a `[3,z,y,x]` deformation field, complex-valued arrays) are **extra value
columns** — the container generalises without a new primitive.

### 3. Why NOT a bespoke sparse array codec

A sparse *codec* inside the array substrate (CSR / CSF / CSF3) could be marginally smaller for
*structured* sparsity, but it would fork every path we already have:

- a parallel **integrity** path (Merkle root over CSR chunks vs Vortex chunks),
- a parallel **stats/monoid** path (nnz / min / max / sum over a bespoke layout),
- a parallel **pruning** path (predicate pushdown into sparse rows).

COO-as-table is **composition over a new primitive** (ADR-0029 §1): the Merkle tree, the
`count`/min/max/sum monoids (ADR-0028), coordinate-range pushdown, streaming MMR append, and — via
#262 — SQL over the table all **already work**. Vortex already compresses runs and dictionary-codes
low-cardinality coordinates. Adopting a sparse codec is a decision the format may revisit if a
future bench shows a *large* on-disk gap at operationally-relevant occupancies; today the empirical
answer is *no*, so the door stays closed pre-1.0.

### 4. Stats, ordering, determinism

- **Monoids fall out for free.** `count` over `value` = `nnz`. `min/max/sum/mean` over `value` are
  the nonzero stats. `count` over the *implied* dense grid = `nnz + (∏shape − nnz)` fill-cells
  (cheap — needs only `shape`, no scan).
- **Canonical row order** = lexicographic by coordinate tuple `(i0, i1, …)`, or ascending `idx` in
  the single-index form. Required (not optional): makes the digest **stable** and gives
  coordinate-range pushdown locality for free.
- `fill_value` is part of identity (it changes what the array *means*) and lives in the manifest
  under `manifest_hash`.

### 5. Materialize-don't-densify (the handoff contract)

Tessera delivers the **coordinate + value columns** zero-copy to Arrow. A `scipy.sparse.coo_matrix`
(or CSR via one downstream conversion) is near-zero-copy. Producing a **dense** numpy array is an
`O(∏shape)` scatter the *consumer* opts into — never forced by the format. A `decode_subset`-style
read returns only the nonzeros inside the region (read-RAM ∝ nnz-in-region, not region volume).
Densification is downstream and optional.

## Consequences

- **No new primitive, no new substrate.** Scatter-sparse is a Vortex table with three extra
  metadata fields (`shape`/`dtype`/`fill_value`); block-sparse is the existing dense-chunked array
  + `count=0` prune. Pure convention.
- **DRY integrity + query** — Merkle, monoid stats, pushdown, streaming append, SQL over tables
  (#262) all apply to the COO table unchanged.
- **Deterministic** — canonical row ordering + explicit `fill_value` → stable digest; the encoding
  choice is stored, never re-inferred at read (mirrors the codec-`auto` rule of #213).
- **The bench answers the "when"** — occupancy is not the decision axis; **materializability +
  access pattern** is (§1 measured rule).

### What's built vs still a gap

- **Built:** `tessera_io::array::to_coo(data, fill)` emits the single-linear-index COO form with an
  explicit `fill`; `ArraySpec.fill_value` is on the array metadata; `count=0` chunk-prune is on the
  array path (ADR-0028); the crossover bench + revised rule (ADR-0031 §5) is recorded.
- **Gap:** the multi-axis `(i0, i1, …, i_{n-1}, value)` emission is a documented extension of the
  convention, not yet a producer path; when it lands it changes only the coordinate width, not the
  decision. Producers today carry the n-D `shape` alongside the single-index form so a reader can
  densify.

## Non-goals

- **No bespoke sparse array codec** (CSR/CSF/CSF3) in the array substrate — rejected by §3 and by
  the measured bench.
- **No in-format sparse compute** (spmv, matmul, solves) — that is scipy/numpy's concern; Tessera
  ends its obligation at cheap zero-copy delivery of coordinates + values.
- **No automated dense↔COO promotion at read time** — the encoding is chosen *once* at write, stored
  explicit, never re-decided (deterministic reads).

## References

- Tracks #218; ratifies ADR-0031 (as-built).
- ADR-0023 (array payload), ADR-0024 (table payload), ADR-0027 (chunk-index stats), ADR-0028
  (unified hierarchy — `count=0` prune), ADR-0029 §6 (substrate-by-nature), ADR-0032 (`fill_value`
  + `unit` on ArraySpec, `Referenced` per-axis), #213 (codec-`auto` "store the concrete answer"),
  #262 (SQL over tables), #221-A (crossover bench).
- Code: `tessera_io::array::to_coo`, `tessera_core::block::array::ArraySpec.fill_value`,
  `tessera_core::chunk_index::prune`.
