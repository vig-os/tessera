---
type: issue
state: closed
created: 2026-06-26T14:37:49Z
updated: 2026-09-28T17:54:16Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/216
comments: 2
labels: area:core
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-09-29T07:53:03.635Z
---

# [Issue 216]: [Data model: composition over inheritance · N-D blocks · multi-dimensional acquisitions (PET-dynamic/diffusion/multi-contrast) · ROI representation](https://github.com/vig-os/tessera/issues/216)

Captures the modeling decisions from the design thread (ADR-0029).

## Decisions
1. **Composition over inheritance.** No product-type class hierarchy. Primitives = N-D array block · table block · manifest spine · typed fields. A product is a *composition* of blocks; the **schema** declares required blocks/fields; a **feature** (timing table, bval/bvec, ROI, pyramid sidecar) is detected by **presence**, not `isinstance`. Evolution is additive. DRY for shared requirements via composable **trait/mixin requirement-sets** (e.g. `imaging_base`), not subclassing.
2. **Rank-agnostic N-D array blocks.** `ArraySpec.shape` is any rank → static `[z,y,x]` and dynamic `[t,z,y,x]` are the *same* primitive at different rank. Static is **not** '4-D with T=1'; dynamic = static **+** composed blocks (time axis + timing table), discovered by presence.
3. **Multi-dimensional acquisitions — A vs B:**
   - **Homogeneous regular extra-axis** (dynamic PET, diffusion @ fixed matrix) → **one N-D array block** chunked `(t_c,64³)` + a **parameter/index table** (frame timing / bval-bvec). Unified access: per-frame + TAC + temporal-projection + spatial pyramid + Merkle are one structure; `t_c` is the per-frame↔TAC locality lever. Append-along-outer-axis = the MMR append (dynamic PET rides the listmode streaming engine; summed static falls out live).
   - **Heterogeneous set** (multi-contrast MRI, mixed geometry) → **separate array blocks** + grouping (manifest `study`/schema) + optional registration sidecar. Forced, not chosen.
   - The `t_c` Vortex (parameter table) exists in both; the fork is only whether the *pixels* are one 4-D array (unified) or N blocks.
4. **ROI = first-class block, representation by nature** (the `roi` schema = 'labels, geometry, statistics'): irregular → raster/label N-D **array** block; primitive (box/sphere) → typed parametric **table**; contours → **vertex table**; stats (mean SUV…) → small table (= image folded under the mask). Provenance-linked (`segments`/`derived_from`); **canonical** (manual) or **derived sidecar** (auto → recipe-stamped, e.g. nnU-Net/Otsu).
5. **New product schemas:** `dynamic_pet` (4-D volume + frame-timing table), `diffusion_mri` (4-D volume + bval/bvec table), `multicontrast_mri` (N volume blocks + per-block contrast metadata), and pin ROI dual-representation in the `roi` schema.

## Done-when
- ADR-0029 accepted; the composition/N-D/A-vs-B/ROI rules in SPEC §5/schema docs.
- Schema registry: `dynamic_pet`/`diffusion_mri`/`multicontrast_mri` + ROI representation; trait/mixin shared requirement-sets.
- Relates ADR-0025 (ingest), ADR-0028 (N-D fold/pyramid/projection).
---

# [Comment #1]() by [gerchowl]()

_Posted on June 26, 2026 at 03:07 PM_

ADR-0029 §6 added: **substrate selection by nature, not rank**. The Vortex-vs-zarr split is grid-vs-rows, not dimensionality — a 1-D dense spectrum is an *array*, a 1-D event list is a *table*. Density is the second lever: dense N-D histogram → array+binning metadata; sparse high-D histogram → COO `(coords…, count)` table. ROOT spans both (TTree→table, TH*→array, TGraph→table). Two orthogonal open areas spun out below.

---

# [Comment #2]() by [gerchowl]()

_Posted on September 28, 2026 at 05:54 PM_

Closing as **done** — verified on `origin/dev` in the 2026-09-28 backlog triage.

Evidence: ADR-0029 accepted (docs/adr/0029-data-model-composition.md status: Accepted, as-built; §6 substrate-selection-by-nature landed c0bd178).

https://claude.ai/code/session_01XdERKMVDAwfMJSKdTytNnK

