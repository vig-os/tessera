---
type: issue
state: closed
created: 2026-07-02T15:30:24Z
updated: 2026-09-28T17:55:00Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/310
comments: 4
labels: feature, discussion, priority:high
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-09-29T07:52:50.285Z
---

# [Issue 310]: [feat(ingest): quantize reconstructed float columns to int+scale — 1.8–2.1× smaller, physically lossless, exact aggregation](https://github.com/vig-os/tessera/issues/310)

## Context — first-user DUPLET FAIR shakedown

The GE listmode `events_*` tables store reconstructed quantities as **float32**: `en` (keV), `vtx`
(mm), `lt`/`lt_corr` (ns). These are **dense, single-scale, bounded** (no multi-decade dynamic range),
and their float32 mantissa carries ~3 orders of magnitude more "precision" than the physical
measurement contains — i.e. noise. Quantizing to **int + a fixed scale** at the physical resolution is
smaller, faster, and *more* numerically correct. Benchmarked on real DP01 data (pcodec, the codec
tessera uses for tables via Vortex/pcodec).

## Benchmark (1–2 M-row samples, pcodec)

| column (physical resolution) | float32 pcodec | int16 + scale pcodec | shrink | quant error vs resolution |
|---|---|---|---|---|
| `en` keV (~50 keV FWHM @ 511) | 5.40 MB | **2.57 MB** (×10 → 0.1 keV) | **2.10×** | 0.05 keV — 1000× under |
| `vtx` mm (~4 mm spatial res) | 10.05 MB | **5.50 MB** (×80 → 0.0125 mm) | **1.83×** | 0.006 mm — 600× under |
| `lt` ns (~0.4 ns TOF res) | 3.29 MB | **1.87 MB** (×1000 → 1 ps) | **1.76×** | 0.5 ps — 800× under |
| `dt` (already integer, 192 vals) | 0.82 MB | 0.81 MB | ~1× | 0 (keep int16 → halves RAM) |

At `events_2p` scale (158 M rows), `en`+`vtx` alone: ~2.4 GB → ~1.3 GB pcodec, **physically lossless**.

## Findings

- **pcodec already partially exploits fixed-point** (FloatMult mode): real floats compress ~2× better
  than random-mantissa floats of the same range (control test). Vortex's **ALP** does the same. **But
  neither matches explicit quantization** (int16+scale is still 1.8–2.1× smaller) — a lossless codec
  cannot discard sub-resolution mantissa noise it isn't told is noise. **The win is a quantize-at-ingest
  domain decision**, not a codec setting.
- **Physically lossless**: errors are 600–1000× below the measurement resolution.
- **Exact aggregation**: summing 158 M float32 values silently drops low bits past ~2²⁴·ULP → biased
  histograms/means. Integer → int64 accumulation is **exact** over billions of rows.
- **Half the memory/bandwidth**: int16 (2 B) vs float32 (4 B) — dominant cost on large scans; native
  zero-copy Arrow.
- **Same principle as the PET-rescale decision (#300)**: quantize to physical resolution as int + a
  scalar scale, applied to the array tier there, the table tier here.

## Compute-engine handling (int + scale, without round-tripping through float)

The efficiency comes from **keeping columns integer through the hot path and factoring the scale out to
the boundary** — engines do *not* multiply-to-float per value:

- **Filter / window** (energy window, TOF gate): translate the predicate to int-space once
  (`en ∈ [425,650] keV` → raw `∈ [4250,6500]`). Exact integer comparison, branch-friendly, half-width.
- **Count / group-by** (per-crystal, per-time): pure integer — scale irrelevant.
- **Linear aggregates** (sum, avg, min, max): compute on raw ints (int64 accumulator, exact), then
  apply the scale **once to the scalar result** — the scale factors out of any linear reduction.
  Variance scales by `scale²`. No per-value float.
- **Scale → float** only at the terminal boundary (display, ratios, sqrt/transcendental) — one
  vectorized `int × 10⁻ⁿ` multiply on the (small) output, not the (large) input.

### Type options
- **Plain `Int16` column + scale in column metadata** (`Column.scale`/quantum, see #307): smallest
  in-memory (2 B), best compression; the *application* manages the scale (predicate translation +
  terminal multiply). Allows arbitrary scales (e.g. `vtx` ×80). Recommended for the hot path.
- **Arrow `Decimal32`/`Decimal64`** (scale as type metadata, engine-native decimal arithmetic): cleaner
  semantics, exact decimal ops, but min width **4 B** (Decimal32) — loses the int16 halving — and
  **power-of-10 scales only** (`en` ×10 ✓, `lt` ×1000 ✓; `vtx` ×80 → use ×100 = 0.01 mm, needs int32
  range for ±370 mm). `Decimal128` (16 B) is larger than float32 — avoid.

## Ask

1. Ingest reconstructed float columns as **int16 + fixed physical-resolution scale**; keep already-int
   columns (`ms`, `dt`, `ax`, `tx`, counts) integer as-is.
2. Add **`Column.scale`/quantum** (+ reuse `unit` from #307) so `raw_int × scale = physical value`.
3. Choose scale with **~20 % headroom** over observed max (int16 = ±32767); **clip + count** rare
   outliers (record the clip count in metadata) — the one discretization risk.
4. Prefer **power-of-10 scales** where they fit int16, to keep an Arrow-`Decimal32` export path open.

Found during: DUPLET first-user FAIR ingest (DP01). Relates to #307 (`Column` annotation/scale),
#305 (listmode table modeling), #300 (array-tier quantization = same principle).

---

# [Comment #1]() by [gerchowl]()

_Posted on July 2, 2026 at 03:31 PM_

## Compute nuance — is float actually slower, and is int→float conversion a cost?

**Per-element ALU: float ≈ int.** Modern CPUs/SIMD run float32 and int add/mul/compare at the same
throughput (float even has FMA + wide vector units); Arrow/DataFusion kernels for both are equally
mature. **The int win is NOT faster arithmetic** — it is:
1. **Bandwidth + SIMD width.** int16 (2 B) vs float32 (4 B) → 2× values per cache line / vector lane
   (AVX-512: 32× int16 vs 16× float32). Listmode analytics over 100 M+ rows are **memory-bandwidth
   bound**, so half the bytes ≈ 2× scan/filter throughput. The win lives on load/scan/filter, not math.
2. **Exact aggregation** (int64 accumulator) — correctness, not speed.

**int→float conversion is cheap but must be contained.** The convert is one SIMD op (`vcvtdq2ps`), but
materializing a whole column to float re-inflates to 4 B and discards the bandwidth win. Rule: keep int
through filter/count/sum; convert only surviving rows or the scalar result.

**Linear ops: scale factors out.** `SUM/AVG/MIN/MAX` compute on raw ints (exact), then multiply the
single result by the scale. Variance scales by `scale²`. No per-value float.

**The footgun: integer division truncates.** `a/b` on raw scaled ints floors → silent wrong answer.
For a ratio of same-scale columns the scale **cancels** (`(a·s)/(b·s)=a/b`) → just `float(a)/float(b)`.
This is why the scale MUST be machine-readable (#307): apply-scale-on-read helpers prevent hand-rolled
int division on quantized columns.

**Net decision matrix (rescale depending on the math):**

| workload | representation |
|---|---|
| filter / window (energy, TOF)   | int (predicate translated to int-space) |
| count / group-by / histogram    | int (scale irrelevant) |
| sum / avg / min / max (linear)  | int, apply scale to the scalar result |
| ratios / sqrt / log (nonlinear) | int → float on surviving rows (scale often cancels) |

Storing int+scale never loses: the ~90 % hot path (window + histogram + group) is faster and exact in
int; the ~10 % nonlinear tail pays one cheap, contained convert on reduced data — where float was needed
anyway.


---

# [Comment #2]() by [gerchowl]()

_Posted on July 2, 2026 at 03:32 PM_

Compute/query-engine handling of scaled-int columns split out to its own issue → see the sibling issue on scale-aware compute (int-space predicates, scale-factoring, apply-scale-on-read). This issue (#310) stays focused on the ingest/storage quantization decision.

---

# [Comment #3]() by [gerchowl]()

_Posted on July 2, 2026 at 03:42 PM_

## Probe 2 — Vortex ALP does NOT close the storage gap (measured, real DP01 data, 2 M rows)

| column | Vortex float32 | Vortex int16+scale | int smaller by |
|---|---|---|---|
| `en` (keV) | 11.47 MB (`vortex.alprd`) | **5.62 MB** (`fastlanes.for`) | **2.04×** |
| `vtx` (mm) | 21.32 MB (`vortex.alprd`) | 12.00 MB (`vortex.primitive`, **uncompressed!**) | 1.78× |

Two findings:
1. **Vortex's ALP (ALP-RD) on these reconstruction floats only gets 1.1–1.4×** — the high-entropy
   mantissa defeats it — so **explicit int16+scale is still ~1.8–2× smaller**. ALP does **not** make the
   quantize-at-ingest decision redundant; the storage win (this issue) holds in the Vortex table path.
2. **Vortex's default int cascade underperforms pcodec on `vtx` int16**: it fell back to
   `vortex.primitive` (raw, 12 MB), whereas pcodec compressed the same column to 5.5 MB (the table
   above in this issue). ⇒ to *realize* the int16 win in the Vortex-backed table path, tessera should
   ensure the **pcodec encoding is used within Vortex** for these columns, not Vortex's default
   FOR/primitive. (Vortex supports a pcodec encoding — needs to be enabled/selected.)


---

# [Comment #4]() by [gerchowl]()

_Posted on September 28, 2026 at 05:54 PM_

Closing as **done** — verified on `origin/dev` in the 2026-09-28 backlog triage.

Evidence: commit 2e3a2ff 'feat(ingest): tessera ingest ge-hdf5 --quantize — opt-in float→int16 transform (#310) (#316)' + supporting ab83ba9.

https://claude.ai/code/session_01XdERKMVDAwfMJSKdTytNnK

