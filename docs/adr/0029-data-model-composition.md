# ADR-0029 — Data model: composition over inheritance · N-D blocks · multi-dimensional acquisitions · ROI

Status: **Accepted** (2026-06-26, as-built) · Tracks `#216` · Relates to ADR-0025 (ingest), ADR-0028
(N-D fold / pyramid / projection), and the product-schema registry.

**As-built** (every section has tested code): §1 composition-over-inheritance + §2 rank-agnostic =
`ArraySpec.shape: Vec<u64>` (no class hierarchy; schemas are embedded data, `schema.rs`). §3 model-A-vs-B
acquisitions = `dynamic_pet` / `diffusion_mri` / `multicontrast_mri` in `builtin_schemas()`
(`dynamic_pet_requires_volume_and_frame_timing`). §4 ROI dual-representation = `roi` schema `kind: None`
(array OR table by nature; `roi_accepts_representation_by_nature_array_or_table`). §5 trait/mixin
requirement-set = `schema::imaging_base()` composed into the imaging schemas, modality defined once
(`imaging_base_trait_set_is_shared_across_modality_schemas`). §6 substrate-by-nature = `BlockKind`
dispatch (1-D dense `spectrum` → array, `listmode` rows → table). Verified by a fresh-context audit.

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

> **IMPLEMENTED (2026-06-26):** the three schemas are in `SchemaRegistry::builtin()` (registry 9→12),
> each requiring its blocks (`dynamic_pet`/`diffusion_mri` = Array volume + Table timing/gradients;
> `multicontrast_mri` = ≥1 Array volume) + a `modality` coded field. Additive — no corpus regen; tests
> `registry_has_all_builtins`, `dynamic_pet_requires_volume_and_frame_timing`. Still pending for this
> §: the ROI dual-representation pin and the trait/mixin requirement-sets (composition DRY).

### 6. Substrate selection — by **nature**, not **rank**
Which of the two primitives a datum lands in is **not** a function of dimensionality. The discriminator is
the datum's *shape of access*:

- **dense regular N-D grid** (indexed by coordinate/region; a value exists at every cell) →
  **N-D array block** (zarr+pcodec), *at any rank* — a 1-D spectrum/waveform, a 2-D image, a 3-D volume,
  an N-D histogram, a matrix. **A 1-D dense signal is an array, not a table.**
- **rows of typed fields** (indexed by column/predicate/row; each row is a heterogeneous tuple) →
  **table block** (Vortex), *at any length* — listmode events, catalogs, point sets, the chunk-index,
  parameter/timing/bval tables.

So the naïve "1-D→table, 3-D→array" rank rule is wrong: a 1-D thing can be **either** (a dense spectrum →
array; an event list → table), and the choice is always grid-vs-rows. The registry already reflects this
(`spectrum` = 1-D dense → array; `listmode` = rows → table).

**Density is the second lever, for grids that are mostly empty:**
- a **dense** N-D histogram (ROOT `TH1/TH2/TH3/THn`) → an **N-D array of bin counts** + a tiny
  **binning-metadata** table/fields (`nbins/lo/hi` or variable edges) — the *same* array+parameter-table
  pattern as `dynamic_pet`;
- a **sparse** high-D histogram (mostly-zero, common in HEP) → a **table of `(bin_coords…, count)`** (a
  COO coordinate list) — a dense grid would be almost all zeros, so density *flips* it back to the table
  substrate.

The cross-ecosystem mapping then falls out by object nature, not source format — e.g. **ROOT** spans both
substrates: `TTree` → table (Vortex), `TH*` → array (+binning), `TGraph`/`TGraphErrors` → table (columns
`x,y,ex,ey`). "ROOT's n-dims are just zarr" holds for its histograms/dense arrays; its TTrees are tables.
The substrate is chosen by *what the data is*, never by which tool emitted it.

> **Open follow-on (not decided here):** the *sparse* representation itself (a COO table convention vs a
> sparse array codec) is its own choice — tracked in `#218`. Likewise **spatial referencing** (voxel→world
> affine + orientation) is an orthogonal open area with its own ADR — tracked in `#217`.

## Consequences
- **No new primitives** — composition reuses the N-D array block + table block + fields we already have;
  the change is *schemas + conventions*, not the substrate. `ArraySpec` is already rank-agnostic.
- **Unified access for regular N-D** falls out of treating the extra dimension as an array **axis** (so
  the ADR-0028 fold/pyramid/projection + the streaming MMR append all apply unchanged in 4-D).
- **SPEC additions:** the composition model + feature-by-presence rule; the homogeneous-vs-heterogeneous
  + `t_c` rule; the ROI representation matrix; the new schemas + trait-composition; the **substrate-by-
  nature** rule (grid→array / rows→table, density-flip for sparse, the histogram + ROOT mapping).
- **Backward-compatible / additive** — existing `recon`/`listmode`/`roi` products are unchanged; the new
  schemas and the N-D usage are additions.

## Status note
Proposed; schemas + trait-composition land with the post-accumulator work. The N-D *mechanics* (chunk
grid, fold, pyramid, projection over the time axis) are already specified in ADR-0028; this ADR is the
*modeling* layer — how products compose and relate.
