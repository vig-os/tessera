---
type: issue
state: open
created: 2026-07-02T14:26:26Z
updated: 2026-09-28T17:59:25Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/301
comments: 0
labels: feature, discussion, effort:medium
assignees: none
milestone: 0.1.0-beta
projects: none
parent: none
children: none
synced: 2026-09-29T07:52:52.784Z
---

# [Issue 301]: [feat(ingest): dicom-series is stack-only — missing primitives for scouts (multi-block), raw non-image DICOM](https://github.com/vig-os/tessera/issues/301)

## Context — first-user DUPLET FAIR shakedown

Ingesting `DUPLET-Patients/DP01` (19 GE series) surfaced that `ingest dicom-series` is **stack-only**
and several real, common series shapes have **no ingest primitive**:

1. **CT scout / localizer** (`1__CT__CT_SCOUT_HEAD_IN`, 2 images = AP + lateral, *different shapes*):
   rejected by `dicom-series` ("mixed shapes"). These are 2 distinct 2-D projections — the natural
   shape is **one `.tsra` with 2 image blocks**, but no primitive emits multi-block-from-heterogeneous
   DICOM. Today you get 2 separate single-block `.tsra` (via single-file `ingest dicom`) or a blob.
2. **Raw non-image DICOM** (`901/902__GEMS_PET_RAW`): no `(0028,0010)` Rows/PixelData → `ingest dicom`
   errors `No such data element (0028,0010)`. These are raw PET (sinogram/projection) objects that
   belong in the **blob** cold tier; needs classification, not a decode primitive.
3. **Per-slice-scaled PET**: see #300 — the primitive assumes a scalar rescale.

## Ask

- A general **array-assembly primitive**: "stack these DICOM objects into N image blocks in one
  product", tolerant of heterogeneous shape (→ separate blocks) and per-slice scale (→ #300).
- Auto-classify non-image DICOM (no pixel grid) → route to `blob` (or a clear error telling the user to).
- Confirm the intended representation for a 2-projection scout (2 blocks in 1 `.tsra`?).

Found during: DUPLET first-user FAIR ingest (DP01).

