---
type: issue
state: closed
created: 2026-07-02T14:26:25Z
updated: 2026-09-28T17:54:50Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/300
comments: 1
labels: feature, discussion, priority:high, effort:medium
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-09-29T07:52:53.149Z
---

# [Issue 300]: [feat(ingest): GE PET per-slice RescaleSlope — recon can't represent quantitative PET (empirical rescale analysis)](https://github.com/vig-os/tessera/issues/300)

## Context — first-user DUPLET FAIR shakedown

Found while ingesting real GE Discovery MI Gen2 PET/CT (`DUPLET-Patients/DP01`) into `.tsra` as a
first full-user run. The clinically-primary PET reconstructions (Q.Clear: `12/13/14/15__PT`) **cannot
be ingested**:

```
error: invalid product: dicom: non-uniform series (shape/modality/rescale differ between slices)
```

## Root cause

GE PET stores each axial slice with its **own** `RescaleSlope` (0028,1053) — the Bq/mL scale that
maximizes each slice's use of the int16 range. `13__PT` = **487 distinct slopes** across 487 slices.
`dicom-series` (and single-file `dicom`) assume **one scalar** slope per volume, and the `recon`
schema models `rescale_slope` as a scalar → the series is rejected. Uniform-slope series (e.g.
`603__PT__PETCoronal`, `2__CT__CTAC`) ingest fine.

The stored **pixels are ordinary int16** (`BitsAllocated=16, PixelRepresentation=1`); only the
per-slice float *scale* is the problem.

## Empirical analysis (13__PT, 487×256×256 = 31.9M voxels, pcodec — the real codec)

Compressed sizes + worst-case error on clinically-meaningful voxels (>100 BQML):

| Representation | size (pcodec) | vs A | abs worst | rel p99 / max |
|---|---|---|---|---|
| raw int16 (uncompressed) | 63.8 MB | — | — | — |
| **A · int16 stored + slope[] (lossless)** | 13.9 MB | 1.0× | **0 (bit-exact)** | 0 |
| **i32 · round(BQML), scale=1** | **10.2 MB** | 0.73× | ±0.5 BQML | 0.21% / 0.50% |
| C · global int16 (one slope) | 7.6 MB | 0.55× | ±1.68 BQML | 0.71% / 1.64% |
| B · float32 real | 28.6 MB | 2.06× | lossy + 2× size | — |
| float16 real | 17.4 MB | 1.25× | **UNUSABLE** (390 voxels → `inf`; BQML range 110k ≫ 65504) | — |

### Key findings
- **float16 overflows** on BQML (range here 110k; up to 4.95M in `PETCoronal`) — non-starter.
  **float32** is 2× the lossless size with no fidelity gain.
- **3D-continuity result (decisive):** per-slice-scaled int16 is **z-discontinuous** (each slice
  independently scaled → artificial jumps across z that fight pcodec's spatial prediction). Storing
  the physically-meaningful **3D-continuous real field** as `int32 round(BQML)` compresses
  **smaller (10.2 vs 13.9 MB)** *and* is near-lossless (±0.5 BQML, p99 rel 0.21%).
- `int32 BQML` is a **plain 3-D array → 64³ chunking unchanged, no slope-array machinery**, single
  interpretation (scale=1). A per-slice `slope[]` sidecar would *also* preserve 64³ (it's a 1-D
  z-length block, orthogonal to the voxel grid) but keeps the z-discontinuous payload.

## Recommendation

Hot-tier `recon` for per-slice-rescale PET = **`int32` real-world BQML** (`round(stored·slope)`),
3-D-continuous, ~10 MB/series, ±0.5 BQML. Bit-exact reproduction of the original vendor bytes is
**already preserved by the cold-tier blob** of the raw DICOM, so the hot tier is free to store the
physical quantity. Optionally derive a display/global-int16 product later (provenance-linked).

Decision needed: (a) `int32 BQML` hot tier [recommended], (b) `int16 + per-slice slope[]` bit-exact
hot tier, (c) both (archival + derived display). Then implement in the DICOM backend (+ recon schema
allows int32 dtype / or a rescale-array block) + gate/test.

Repro data: `/mnt/HDD/data/sdsc_dump/GEDiscoveryMIGen2/Projects/DUPLET-Patients/DP01/13__PT__TK_Q.Clear_450`

---

## DECISION (2026-07-02) — global int16, one slope

Chosen: **global int16 rescale** (option C) for the PET hot tier.

**Rationale:** worst-case relative error is **1.64%** on clinically-meaningful voxels (>100 BQML),
p99 = 0.71% — far below PET's inherent quantitative noise (SUV test–retest ≈ 10–20%), and negligible
on top of background. Bit-exact vendor bytes remain recoverable from the **cold-tier blob** of the raw
DICOM, so nothing is truly lost.

**Why it's also the cleanest build:**
- Fits the **existing scalar `rescale_slope`** field — no int32 dtype, no per-slice `slope[]` array,
  no schema change.
- Global-int16 quantizes the **3-D-continuous real field** → 64³ chunking clean, and it's the
  **smallest** representation measured (7.6 MB vs 13.9 MB for per-slice-scaled int16).

**Implementation (DICOM backend):** when slices share shape+modality+intercept but differ in slope,
compute `global_slope = max(|real|)/32767`, store `round(real/global_slope)` as int16 with that one
scalar slope, instead of rejecting.

**Implementation note:** `global_slope` is set by the single hottest voxel, so an extreme hot spot
(injection site / bladder) inflates the step and coarsens the rest. `max` is the safe bounded choice;
a robust high-percentile cap (clipping the hottest outliers) is a possible refinement if needed.

## Emit the requantization as metadata (observability on ingest)

A lossy normalization must be **self-documenting** — otherwise a consumer can't tell the hot-tier
volume was requantized, by how much, or how to invert it. On ingest, the backend must:

**1. Emit a WARN** (same channel as the existing PHI/`recommended`-field warnings), e.g.:
> `WARN lossy rescale: 487 distinct per-slice RescaleSlopes collapsed to one global int16 scale
> (step=3.36 BQML, max abs err ±1.68 BQML). Per-slice slopes preserved in metadata; raw DICOM in blob.`

**2. Write transform provenance into the product metadata:**
```
rescale_mode            = "global-int16"      # vs "bit-exact" for uniform-slope series
rescale_units           = "BQML"
rescale_slope           = <global_slope>      # the scalar (existing recon field)
rescale_slope_per_slice = [<n_z original slopes>]   # ~1.9 KB — makes the transform INVERTIBLE
rescale_quant_step_bqml = <global_slope>
rescale_max_abs_err_bqml= <global_slope/2>
rescale_lossy           = true
```

Preserving `rescale_slope_per_slice` means a consumer can reconstruct the original per-slice scaling
(and cross-check against the cold blob) **without** re-reading 120 GB of raw DICOM. It's also a general
principle: **any lossy transform applied at the door emits its parameters + a warning + the inverse.**

---

# [Comment #1]() by [gerchowl]()

_Posted on September 28, 2026 at 05:54 PM_

Closing as **done** — verified on `origin/dev` in the 2026-09-28 backlog triage.

Evidence: commit 9e8c289 'feat(ingest): dicom-series --rescale-mode global-int16 — quantitative PET per-slice rescale (#300)'.

https://claude.ai/code/session_01XdERKMVDAwfMJSKdTytNnK

