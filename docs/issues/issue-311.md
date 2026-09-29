---
type: issue
state: open
created: 2026-07-02T15:32:22Z
updated: 2026-09-28T18:00:05Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/311
comments: 1
labels: feature, discussion
assignees: none
milestone: backlog / research
projects: none
parent: none
children: none
synced: 2026-09-29T07:52:49.950Z
---

# [Issue 311]: [feat(query): scale-aware compute over quantized int+scale columns — int-space predicates, scale-factoring, apply-on-read](https://github.com/vig-os/tessera/issues/311)

## Context

Split out from #310 (which is about the **ingest/storage** decision: quantize reconstructed float
columns to int + fixed scale). This issue is the **compute/query-engine** layer: how DataFusion /
Arrow / Vortex should operate over quantized `int + scale` columns so the storage win (½ bandwidth,
exact aggregation) is actually realized at query time — and the footguns avoided.

Storage decision lives in #310; the column `scale`/`unit` metadata lives in #307. This issue is purely
the engine/query semantics.

## The principle: keep int through the hot path, factor the scale to the boundary

Per-element ALU throughput for float32 ≈ int (both fully optimized) — **the int win is bandwidth + SIMD
width (2× on memory-bound scans) and exact int64 aggregation, not faster arithmetic.** To capture it,
the engine must not silently materialize columns to float.

| workload | engine should… |
|---|---|
| filter / window (energy, TOF) | translate predicate to **int-space** once (`425 keV`→`4250`); compare raw ints |
| count / group-by / histogram | operate on raw ints; scale irrelevant |
| sum / avg / min / max (linear) | accumulate raw ints (int64, exact); apply scale **once** to the scalar result (variance ×`scale²`) |
| ratios / sqrt / log (nonlinear) | convert **surviving rows only** to float; for same-scale ratios the scale **cancels** |

## Footgun to guard

**Integer division truncates** — `a/b` on raw scaled ints floors → silent wrong result. Engine/UX must
route physical division through float (or a scale-aware op), never raw int-divide a quantized column.

## Ask (engine-side)

1. **Apply-scale-on-read helpers / logical columns**: a `LogicalTableView`/projection that exposes the
   physical (float) value `raw × scale` on demand, so users never hand-roll the multiply or int-divide.
   Reads `scale`/`unit` from column metadata (#307).
2. **Int-space predicate pushdown**: window filters expressed in physical units get translated to the
   raw-int domain before pushdown to Vortex/pcodec (keeps prune + scan at int width).
3. **Scale-factoring for linear aggregates**: `SUM/AVG/MIN/MAX` computed on raw ints, scale applied to
   the result — exact and bandwidth-optimal.
4. **Decimal path (optional)**: evaluate exposing power-of-10-scaled columns as Arrow `Decimal32/64`
   for engine-native decimal arithmetic (trade: min 4 B width vs plain `Int16`'s 2 B — see #310).
5. **Guard rails**: reject/warn on raw integer division of a `scale`-annotated column.

## Open questions

- Does DataFusion's optimizer already push a `physical = raw×scale` projection below aggregation, or
  would it materialize float first? (Needs a probe — determines whether (3) is automatic or manual.)
- Vortex `Decimal`/ALP interplay: if Vortex stores ALP-encoded floats, does a scale-aware int path even
  help, or does ALP + pushdown already get there? (Benchmark in #310 says explicit int is 1.8–2.1×
  smaller on disk; the *compute* question is separate and unmeasured here.)

Relates to #310 (storage/quantization), #307 (`Column.scale`/unit metadata), #305 (listmode modeling).
Found during: DUPLET first-user FAIR ingest (DP01).

---

# [Comment #1]() by [gerchowl]()

_Posted on July 2, 2026 at 03:42 PM_

## Probe results (measured) — corrects two of my earlier claims

### Probe 1 — DataFusion optimizer + in-memory int-vs-float (20 M rows, in-memory Arrow)

**The optimizer does NOT algebraically factor the scale.** It executes exactly what you write:

```
SELECT SUM(en_i16)*0.1        →  ProjectionExec: CAST(sum(en_i16)) * 0.1   ← scale on the SCALAR result
                                   AggregateExec: aggr=[sum(en_i16)]         ← int64 accumulation (exact)
SELECT SUM(CAST(en_i16 AS DOUBLE)*0.1)  →  AggregateExec: aggr=[sum(en_i16 * 0.1)]  ← per-ROW float multiply
```

⇒ ask (3) here is **necessary, not automatic**: the apply-scale-on-read layer must emit the
`SUM(raw)*scale` form for linear aggregates; naive `SUM(raw*scale)` materializes float per row.

**Timing (min of 5, ms):** SUM int16 = **10.8**, SUM float32 = **10.8**, SUM(int16*0.1) per-row = 11.3;
filter+COUNT int16 = 12.2, float32 = 13.0. **In-memory compute is ~equal** — int is *not* 2× faster
here (DataFusion widens int16→int64 for SUM; at 20 M rows it's not DRAM-bandwidth-saturated, and the
in-memory path skips decode). **Correction to my earlier note: the int win is disk/decode + footprint +
exact aggregation, NOT faster in-memory arithmetic.** The compute kernels for int and float are equally
optimized.

### Probe 2 (compute-relevant) — Vortex encodings chosen
`en` float32 → `vortex.alprd`; `en` int16 → `fastlanes.for` (bit-packed). Filter/take pushdown on a
bit-packed FOR int column is generally cheaper than on ALP-split floats, but **this was not timed** —
left as a follow-up if the compute path becomes hot.


