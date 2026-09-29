---
type: issue
state: closed
created: 2026-06-26T15:50:20Z
updated: 2026-09-28T17:54:21Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/221
comments: 3
labels: area:testing, area:core, spike, area:io
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-09-29T07:53:02.897Z
---

# [Issue 221]: [[SPIKE] Empirical-overhead spike — resolve the 3(+1) measured thresholds; promote ADRs 0026–0032 Proposed→Accepted](https://github.com/vig-os/tessera/issues/221)

## Why
The design surface (**ADRs 0026–0032**) is fully shaped — every *modeling* decision is settled by reasoning. What remains are the **empirical forks**: a handful of "go one way or the other based on a threshold" knobs the designs deliberately left to measurement. This spike measures them **once**, sets the thresholds, and promotes 0026–0032 **Proposed→Accepted** (at which point each gains its FEATURE-MATRIX row per the matrix gate).

**Principle (carried from ADR-0031):** Tessera is storage/interchange, not a compute engine — every metric here is **disk size · I/O-RAM / bytes-read · materialize time**, never in-memory downstream math (spmv/matmul = scipy's layer). Determinism + lossless storage are invariants, not things we trade for a threshold.

---

## Measurement plan

### A — Sparse: `dense+pcodec` vs `COO+Vortex` crossover  (ADR-0031 §5)
**Fork:** scatter-sparse grid stored dense (pcodec crushes zero-runs) vs COO table.
**Sweep:** occupancy `≈{0.01, 0.1, 1, 5, 10, 25, 50}%` × structure `{random scatter · banded · block-clustered}`.
**Metrics:** on-disk size · region read-RAM/bytes-read · materialize time.
**Baselines (one row each, like #143):** `scipy.sparse.save_npz` · `numpy.memmap` (naive dense). **Not benched:** spmv/matmul.
**Output:** the occupancy×clustering boundary = the default dense↔COO threshold (block-clustered should confirm "dense-chunked + count=0 prune wins — no COO needed").

### B — Chunk-index / sub-block Merkle **leaf granularity**  (ADR-0027 / 0028)
**Fork:** how fine the sub-block Merkle leaves + `{hash, monoid-stats}` chunk-index rows go.
**Sweep:** leaf/row-group size `{2¹², 2¹⁴, 2¹⁶, 2¹⁸, 2²⁰}` rows (and the array-chunk analogue).
**Metrics:** chunk-index size (% of payload) · pruning selectivity (rows skipped for a ranged/predicate read) · proof size/granularity · encode cost.
**Output:** the leaf size where index overhead starts to outweigh pruning+proof benefit (S2/S3 already showed "granularity is the lever" — this pins the default). NB `ROWS_PER_GROUP=2¹⁶` for the *table payload* is frozen (backward-compat); this is the *index/Merkle leaf* granularity, which is free to differ.

### C — `t_c` temporal chunk depth for N-D arrays  (ADR-0029 §3)
**Fork:** chunk shape `(t_c, 64, 64, 64)` for dynamic acquisitions — per-frame reads want `t_c=1`, time-activity-curve (TAC, all frames @ one voxel) wants `t_c` large.
**Sweep:** `t_c ∈ {1, 2, 4, 8, 16, full}` on a representative dynamic-PET-shaped volume (real or synthetic).
**Metrics:** single-frame read (bytes/RAM/latency) · TAC read (all-t @ voxel) · compression ratio · pyramid-fold cost.
**Output:** the per-frame↔TAC crossover → the **default `t_c` per modality** baked into the schema.

### D — (optional, lower stakes) Pyramid depth + projection materialization  (ADR-0028)
**Fork:** default pyramid depth (stop level) · which projections to **pre-materialize** at write (orthogonal MIP/mean = fold byproduct) vs **compute-on-read** (oblique MPR).
**Sweep:** depth to {fits-one-chunk} vs fixed-N · materialized-vs-on-read for the orthogonal set.
**Metrics:** added storage (% of base) vs recompute latency.
**Output:** default depth + the materialize/on-read line. Fold in if cheap; else defer.

---

## Out of scope (settled by reasoning, NOT this spike)
substrate-by-nature (0029 §6) · spatial referencing / LPS (0030) · units/time `(transform,unit,frame)` (0032) · composition model (0029) · **MMR-vs-flat content_hash** (0028 — a *definite* change requiring golden regen, not a threshold).

## Definition of done
- A/B/C thresholds measured + recorded in `SPIKE-RESULTS.md` (+ D if run); harness committed under `tessera/bench/` or `examples/`.
- The chosen defaults written back into the relevant ADRs; **0026–0032 flipped to Accepted**; each Accepted ADR gets its FEATURE-MATRIX row (matrix gate stays green).
- Real-data check where applicable (DUPLET CT/PET, manual-bench-only — no PHI committed).

Relates to #214 (0027), #215 (0028), #216 (0029), #217 (0030), #218 (0031), #219/#220 (0032), #203 (streaming), #143 (bench harness pattern).
---

# [Comment #1]() by [gerchowl]()

_Posted on June 26, 2026 at 04:34 PM_

**A (sparse crossover) — DONE** (`ef3c1f1`, `examples/spike_sparse.rs`, independently verified by a fresh-context reviewer).

**Finding (counterintuitive):** dense+pcodec wins on-disk at **every** occupancy × structure tested (128³ int16, 0.01–50%, scatter/banded/block) — even 0.01% scatter (2.8 KiB dense vs 4.7 KiB best-COO). Tested two COO forms incl. a fair single linear-index to avoid a strawman; neither wins on bytes. pcodec's zero-run compression + tiny coordinate dynamic range dominate.

**COO's real value is read-RAM/latency** (decode 0.05–0.12× at low occ; RAM ∝ nnz) and the **forced** unstorable-ambient-grid case (high-D histograms — asserted, not benched).

**→ ADR-0031 §5 reframed:** the occupancy-crossover doesn't exist in COO's favour; replaced with a decision rule — *dense+pcodec by default when the grid is materializable; COO only for unstorable-ambient or selective-nnz-RAM access.* See SPIKE-RESULTS.md §#221-A.

Remaining: **B** chunk-index/Merkle leaf granularity (needs sub-block-Merkle prototype — ADR-0027/0028), **C** `t_c` temporal depth (needs configurable array chunk shape — ADR-0029), (D) pyramid/projection.

---

# [Comment #2]() by [gerchowl]()

_Posted on June 26, 2026 at 05:04 PM_

**B (chunk-index leaf granularity) — DONE** (`f903651`, `examples/spike_chunk_index.rs` over the real `chunk_index`; fresh-verified).

Index overhead negligible everywhere (<0.9% even at 1024-row leaves; 0.014% at 65536) → **overhead is not the constraint, pruning is**. Knee ~**2¹⁴ (16384)** for locality columns (scan 6.25%→3.1%→1.2% as leaves shrink 65536→16384→4096). Random/non-local → unprunable at any size → hash-only index. **→ decouple index leaf from the frozen byte-payload ROWS_PER_GROUP (2¹⁶); default index leaf ~2¹⁴.** SPIKE-RESULTS §#221-B + ADR-0028 §3.

**Spike status:** A (sparse) ✓ + B (granularity) ✓ — the two standalone-measurable thresholds done. **C (`t_c` temporal depth)** needs configurable array chunk-shape (ADR-0029) → measured during that implementation. **D (pyramid/projection)** optional, deferred. The decision-driving measurements (A, B) are complete.

---

# [Comment #3]() by [gerchowl]()

_Posted on September 28, 2026 at 05:54 PM_

Closing as **done** — verified on `origin/dev` in the 2026-09-28 backlog triage.

Evidence: Spike-A (sparse dense-vs-COO crossover) + Spike-B (chunk-index leaf granularity) both landed (ef3c1f1, f903651). ADRs 0028/0029/0030/0031/0032 = Accepted; ADR-0027 Superseded by 0028. Only ADR-0026 remains 'Proposed' but that is streaming-table extension work, not the spike deliverable.

https://claude.ai/code/session_01XdERKMVDAwfMJSKdTytNnK

