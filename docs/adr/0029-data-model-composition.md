# ADR-0029 — Data model: composition over inheritance · N-D blocks · multi-dimensional acquisitions · ROI

Status: **Proposed** (2026-06-26) · Tracks `#216` · Relates to ADR-0025 (ingest), ADR-0028 (N-D
fold / pyramid / projection), and the product-schema registry.

## Context
Real acquisitions are multi-dimensional and varied: static and dynamic PET, diffusion MRI (per-direction
volumes + b-values/vectors), multi-contrast MRI (T1/T2/FLAIR), ROIs/segmentations. The question is how
the model represents and *relates* these without a brittle type hierarchy — the SSoT/DRY/SOLID question.

## Decision

### 1. Composition over inheritance
There is **no product-type class hierarchy**. The model is a few **primitives** — an **N-D array block**,
a **table block**, the **manifest spine**, and **typed fields** — and everything else is composition:
- a **product** is a *composition* of blocks;
- a **schema** *declares* the required blocks/fields of a product (engine stays domain-agnostic);
- a **feature** (a frame-timing table, a bval/bvec table, an ROI, a pyramid sidecar) is **detected by
  presence**, never by `isinstance` — a tool that needs timing checks "is there a timing table?";
- **evolution is additive** (new field / block / schema, never breaking — the registry rule);
- **DRY for shared requirements** uses composable **trait/mixin requirement-sets** (e.g. an
  `imaging_base` set: `modality`, geometry, patient) *composed into* `recon` / `dynamic_pet` / `roi`,
  not subclassed.

So "inheritance of things/classes/features" is *what blocks a product composes + what its schema
requires + what provenance edges it carries* — never a subclass tree.

### 2. Rank-agnostic N-D array blocks
`ArraySpec.shape` is any rank, so a **static** volume `[z,y,x]` and a **dynamic** one `[t,z,y,x]` are the
**same primitive at different rank**. Static is **not** "4-D with T=1" (no vestigial axis on the common
case); dynamic = static **plus** composed blocks (the time axis + a timing table), discovered by presence.

### 3. Multi-dimensional acquisitions — model A vs B
- **Homogeneous, regular extra-axis** (dynamic PET, diffusion at fixed matrix) → **one N-D array block**
  chunked `(t_c, 64, 64, 64)` + a **parameter/index table** (frame timing; bval/bvec). This is the
  **unified-access** choice: per-frame reads, time-activity curves (TAC), temporal projections (the
  static = a `sum`-over-time fold), the spatial pyramid, and the Merkle/stats tree all operate on **one**
  structure; **`t_c`** is the single per-frame↔TAC locality lever (set per modality by the schema).
  Appending a frame is an **append along the outer axis** — the *same* MMR append as listmode rows
  (ADR-0028), so dynamic PET rides the listmode streaming engine and the **summed static image falls out
  live**.
- **Heterogeneous set** (multi-contrast MRI — different matrix/voxel/dtype) → **separate array blocks**
  (one per contrast) + grouping via the manifest (`study` + schema) + an optional registration sidecar.
  This is **forced** (they can't share a 4-D grid), not chosen.
- The parameter/`t_c` table exists in **both**; the only fork is whether the *pixels* are one N-D array
  (unified) or N blocks (catalogued). For the regular case, the pixels want to be one array.

### 4. ROI — first-class block, representation by nature
The `roi` schema ("Regions of interest — labels, geometry, statistics") already anticipates the duality.
An ROI is a first-class block whose representation is chosen by the ROI's nature:
- **irregular** (tumor, organ) → a **raster/label N-D array block** (binary / `uint16` label map) — rides
  the array machinery (its `count` stat *is* the ROI volume; the image folded under the mask *is* the
  measurement);
- **primitive** (box, sphere, ellipsoid) → a **typed parametric definition** (a small table/struct:
  `type + params`), resolution-independent;
- **contours** (RTSTRUCT-style) → a **vertex table** `(roi_id, slice, x, y)`;
- **statistics** (mean SUV…) → a small table.

ROIs are **provenance-linked** to the image they segment (`segments` / `derived_from` edge) and are
**canonical** (radiologist-drawn) or a **derived sidecar** (auto-segmented → `recipe`-stamped:
`nnU-Net vX` / `Otsu`, regenerable; ADR-0028 sidecar class).

### 5. New product schemas
Add to the registry (additive): `dynamic_pet` (4-D volume `(t_c,64³)` + frame-timing table),
`diffusion_mri` (4-D volume + bval/bvec table), `multicontrast_mri` (N volume blocks + per-block contrast
metadata). Pin the ROI dual-representation in the `roi` schema. Introduce composable trait/mixin
requirement-sets so shared imaging fields are defined once.

## Consequences
- **No new primitives** — composition reuses the N-D array block + table block + fields we already have;
  the change is *schemas + conventions*, not the substrate. `ArraySpec` is already rank-agnostic.
- **Unified access for regular N-D** falls out of treating the extra dimension as an array **axis** (so
  the ADR-0028 fold/pyramid/projection + the streaming MMR append all apply unchanged in 4-D).
- **SPEC additions:** the composition model + feature-by-presence rule; the homogeneous-vs-heterogeneous
  + `t_c` rule; the ROI representation matrix; the new schemas + trait-composition.
- **Backward-compatible / additive** — existing `recon`/`listmode`/`roi` products are unchanged; the new
  schemas and the N-D usage are additions.

## Status note
Proposed; schemas + trait-composition land with the post-accumulator work. The N-D *mechanics* (chunk
grid, fold, pyramid, projection over the time axis) are already specified in ADR-0028; this ADR is the
*modeling* layer — how products compose and relate.
