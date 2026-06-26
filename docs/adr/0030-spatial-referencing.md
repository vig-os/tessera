# ADR-0030 — Spatial referencing: voxel→world affine + named frame, OME-Zarr transforms derived

Status: **Proposed** (2026-06-26 — a fresh-context audit caught a premature "Accepted" flip and reverted
it: §1/§2/§4/§6/§7 + the §3 `at_level` *derivation* primitive + §5 *rigid* registration are as-built, but
§3 OME-Zarr *export* and §5 *deformable* warps + the `deformation_field` schema are **not** — see Status
note) · Tracks `#217` · Relates to ADR-0025 (ingest normalise-at-the-door),
ADR-0028 (multiscale pyramid — per-level transforms are *derived* here), ADR-0029 (composition,
feature-by-presence, rank-agnostic N-D axes), and the product-schema registry. **Generalised by ADR-0032**
— this spatial affine is the `affine_nd` instance of the one `(transform, unit, frame)` descriptor; the
specifics here (LPS, OME-Zarr derivation, registration provenance) stand unchanged.

## Context
A Tessera array block currently carries **axes + units** (`ArraySpec`), but not the **geometry** that
makes an image a physical object: the **voxel→world** mapping (position, orientation, anisotropic
spacing, handedness). Without it a CT volume is an index grid — you cannot place an ROI in patient
space, overlay PET on CT, or record that a registration changed the frame. This is the load-bearing gap
for any real imaging product.

Three established conventions express the same mapping differently, and a fourth is our own pyramid:
- **DICOM** — `ImagePositionPatient` (IPP) + `ImageOrientationPatient` (IOP) + `PixelSpacing`, per slice,
  in an **LPS** patient frame (x→Left, y→Posterior, z→Superior).
- **NIfTI** — `sform`/`qform` as 4×4 affines + a space code (scanner/aligned/talairach/mni), in **RAS**.
- **OME-Zarr / NGFF** — an ordered `coordinateTransformations` list (scale, translation, …) **per
  multiscale level**, extensible.
- **Tessera ADR-0028** — a `{hash,stats}` pyramid built by 2× folds; each level has its own spacing.

The fork is *which representation is canonical* and *how it stays coherent across the pyramid* without
storing redundant per-level geometry (SSoT with ADR-0028).

## Decision

### 1. Canonical geometry = one voxel→world **affine** per spatially-referenced array
A spatially-referenced array carries an optional **`world_frame`** on its `ArraySpec`:
```
world_frame = {
  affine:     [12 × f64],          # 3×4 row-major: [R | t]; implicit last row [0,0,0,1]
  convention: "LPS",               # axis handedness of the world frame (LPS canonical; see §6)
  unit:       "mm",                # UCUM; world-coordinate unit (default mm)
  space:      "patient",           # named target frame: patient | scanner | aligned | atlas:<id>
}
```
The affine maps the **index vector in declared-axis order** to world: for `axes = [z,y,x]` it acts on
`[i_z, i_y, i_x, 1]ᵀ → [x, y, z, 1]ᵀ`. Tying the columns to `ArraySpec.axes` makes it self-describing and
independent of C/F storage order. The affine is a **lossless superset** of all three source conventions
(NIfTI sform is already this; DICOM IPP/IOP/spacing compose into it; OME-Zarr scale+translation is the
diagonal+offset special case).

### 2. Spacing is **derived from the affine**, never stored twice (DRY/SSoT)
Per-axis voxel spacing = the **column norms** of `R`; direction cosines = the normalised columns;
origin = `t`. We do **not** store a separate `PixelSpacing`/`spacing` field — it would be a second source
of truth that can drift. Anisotropy, oblique orientation, and flips are all *in the affine*. The existing
`ArraySpec.axes` units must agree with `world_frame.unit` (gate-checkable later).

### 3. Pyramid coherence — per-level transforms are **derived**, not stored (SSoT with ADR-0028)
ADR-0028 builds level *L* by a 2× fold. Only the **base (level-0) affine** is stored. The level-*L*
geometry is *computed*:
```
spacing_L = 2^L · spacing_0
origin_L  = origin_0 + R_0 · ( (2^L − 1)/2 · 1⃗ )      # block-centre half-voxel shift
A_L       = A_0 ∘ scale(2^L)  with the origin shift above
```
The `(2^L−1)/2` term is the centroid offset of a 2^L block (a downsampled voxel's centre sits at the
centre of the base voxels it averages) — the classic OME-Zarr per-level `translation`. Storing per-level
affines would duplicate this; instead the writer **emits** OME-Zarr `coordinateTransformations` per level
on export (`scale` = `spacing_L`, `translation` = level offset), all derived from `A_0`. One source of
truth; the pyramid stays coherent by construction.

### 4. Spatial axes only — non-spatial axes are referenced by their parameter tables
For N-D acquisitions (`[t,z,y,x]` dynamic PET, `[dir,z,y,x]` diffusion), the affine references the
**spatial sub-axes** (`z,y,x`); the non-spatial axes (time, b-value, contrast) are located by their
**parameter/index tables** (ADR-0029 §3), not by the affine. `world_frame` names which axes are spatial
(default: the trailing 3 of declared order). The geometry is thus orthogonal to the time/parameter axis —
the same affine applies to every frame of a 4-D block.

### 5. Registration = a provenance-rooted `transform` product carrying the new frame
A registration that resamples or reorients data **does not mutate** a block — it produces a new
`transform` product (ADR-0025 provenance-rooted) whose array carries the **new** `world_frame`, plus a
provenance edge recording **source `space` → target `space`** and the registration **recipe**
(`rigid` / `affine` / `deformable`, with the parameters):
- **linear** (rigid/affine) registration → the new affine fully captures it; the recipe stores the 4×4.
- **deformable** (non-linear) warp → the affine is insufficient; the warp is carried as a
  **deformation-field array block** (a `[3,z,y,x]` vector field — same array machinery, its own
  `world_frame`) referenced by the provenance edge. ADR-0029 composition handles this with no new
  primitive.
This makes "which space is this in, and how did it get there" a first-class, verifiable edge rather than
buried metadata.

### 6. World-frame handedness — **LPS canonical** (decided)
Medical ingest is DICOM-dominated (LPS); neuroimaging tools are RAS. The two differ only by a **lossless
sign-flip on the first two world axes** (`A_RAS = diag(−1,−1,1,1)·A_LPS`) — they label the *same* physical
points, so this is a normalisation, not a loss. **Decision: LPS is the canonical world convention.** RAS
sources are **normalised at the door** (ADR-0025) — the common DICOM path is then a no-op; only the rarer
RAS source flips. This pays the conversion **once at ingest** instead of pushing a check-and-reconcile
onto every consumer of every array pair (the N² burden the format exists to delete). The `convention`
field is stored regardless, so an affine is **never** ambiguous, the source frame is trivially
reconstructible (flip back), and a future RAS-canonical choice would be a pure relabelling, not a
migration. (Alternative B — store the source's convention verbatim — is rejected as the default; it suits
only a "preserve source geometry byte-for-byte" stance, and an ingest may still opt out per-array since
`convention` keeps such reads unambiguous.)

### 7. Presence = referenced; absence = index-space (ADR-0029 feature-by-presence)
`world_frame` is **optional**. An array without it is **unreferenced** (valid — a sinogram, a raw
detector grid, a synthetic volume); a tool needing geometry checks "is there a `world_frame`?", never a
type. Adding geometry to an existing array is an **additive** schema change — no corpus regen, no break.

## Consequences
- **No new primitive** — `world_frame` is metadata on the existing array block; deformation fields reuse
  the array block; registration reuses the `transform` product + provenance edge. Pure schema/convention.
- **Lossless interop** — DICOM IPP/IOP/spacing, NIfTI sform, and OME-Zarr scale/translation all map onto
  the affine without loss; export to OME-Zarr is mechanical (§3) and to NIfTI is the affine verbatim.
- **SSoT geometry** — spacing/orientation/origin live **once** (the affine); per-level pyramid transforms
  and per-axis spacing are *derived*, so they cannot drift from the base (DRY, coherent with ADR-0028).
- **Deterministic** — the affine is stored f64 in the manifest (canonical JCS), part of `manifest_hash`;
  no seal-time float reduction, so no cross-env reduction concern.
- **SPEC additions:** the `world_frame` schema (affine/convention/unit/space + spatial-axis tagging); the
  derived per-level OME-Zarr transform formula; the registration→`transform`-product + deformation-field
  rule; the LPS-canonical / normalise-at-the-door convention.
- **Schema touch-points:** `recon` and the imaging-base trait-set (ADR-0029 §5) gain optional
  `world_frame`; `transform` records source/target `space` + recipe; a `deformation_field` schema is
  added for non-linear warps.
- **Gate (future):** once Accepted, a check that `world_frame.unit` agrees with the spatial `ArraySpec`
  axis units, and that the affine is non-degenerate (non-zero column norms).

## Status note
**Implementation status:** §1 `world_frame` descriptor (3×4 affine + convention + unit + space) on
`ArraySpec`, §2 spacing-derived-from-affine (`WorldFrame::spacing` = column norms), and §6 LPS-canonical
convention field are **implemented** (2026-06-26, `tessera_core::block::array`; additive — `Option`,
skip-if-none → existing corpus unchanged; tests `world_frame_spacing_is_derived_from_affine_columns`,
`array_spec_world_frame_is_additive_and_optional`). **Pending:** §3 OME-Zarr per-level transform
derivation (rides the ADR-0028 pyramid), §5 registration-as-`transform`-product + deformation-field block.

Proposed; all decision points settled (§6 LPS-canonical confirmed by the user 2026-06-26). Lands with the
imaging-base trait-set + new schemas (ADR-0029 post-accumulator work); the OME-Zarr per-level derivation
rides the ADR-0028 pyramid. Moves to **Accepted** with the empirical-overhead spike that promotes the
0026–0029 design set (at which point `world_frame` must be cited in FEATURE-MATRIX per the matrix gate).
