---
type: issue
state: closed
created: 2026-06-26T15:07:59Z
updated: 2026-09-28T17:54:19Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/217
comments: 3
labels: area:core, area:imaging
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-09-29T07:53:03.286Z
---

# [Issue 217]: [Spatial referencing: voxel→world affine + orientation (DICOM IPP/IOP · NIfTI sform/qform · OME-Zarr coordinateTransformations)](https://github.com/vig-os/tessera/issues/217)

## Open design area (spun out of ADR-0029)

Tessera carries array **axes + units**, but not yet a full **spatial frame**: the voxel→world **affine** and **orientation** that real imaging needs (so a volume's geometry survives roundtrip and registration is meaningful).

### The modeling fork
Three established conventions express the same thing differently:
- **DICOM** — ImagePositionPatient (IPP) + ImageOrientationPatient (IOP) + PixelSpacing (per-slice).
- **NIfTI** — `sform`/`qform` 4×4 affines (+ codes for the world space: scanner/aligned/talairach/mni).
- **OME-Zarr** — `coordinateTransformations` (ordered scale+translation, extensible) per multiscale level.

### Decision needed
1. **Canonical representation** — a per-array 4×4 voxel→world **affine** (NIfTI-like, lossless superset) vs an OME-Zarr ordered-transform list (composes cleanly with the multiscale pyramid — each level its own scale). Likely: store the affine **and** emit OME-Zarr transforms per pyramid level (ADR-0028) so the geometry rides the existing fold.
2. **World-space identity** — name the target frame (scanner/patient/atlas) so `derived_from`/registration edges are well-typed.
3. **Pyramid coherence** — level-L transform = level-0 affine ∘ 2^L scale (must be derivable, not stored redundantly → SSoT with ADR-0028).
4. **Provenance** — a registration that changes the frame is a `transform` product with the new affine; the edge records source+target frames.

### Why it's load-bearing
Any real PET/CT/MR product is geometrically meaningless without this; it's the prerequisite for ROI-in-world, multi-modal overlay, and registration provenance. Belongs in its own ADR (relates to 0028 multiscale, 0029 composition).

cc ADR-0029 §6 open-follow-on.
---

# [Comment #1]() by [gerchowl]()

_Posted on June 26, 2026 at 03:18 PM_

Drafted as **ADR-0030** (`docs/adr/0030-spatial-referencing.md`, commit `20c1d62`, Proposed).

**Decision:** one voxel→world 4×4 **affine** per spatially-referenced array (lossless superset of DICOM IPP/IOP/spacing · NIfTI sform · OME-Zarr scale/translation), tied to the declared axis order. Key SSoT points:
- **spacing/orientation/origin live once** in the affine (column norms = spacing) — no separate `PixelSpacing` to drift;
- **per-level OME-Zarr `coordinateTransformations` are derived**, not stored — level-L = A₀ ∘ scale(2^L) + the block-centre half-voxel shift — so the ADR-0028 pyramid stays coherent by construction;
- affine references **spatial sub-axes only**; time/bval/contrast located by their parameter tables (ADR-0029 §4);
- **registration = a provenance-rooted `transform` product** carrying the new frame + source→target `space` edge; deformable warps ride a `[3,z,y,x]` deformation-field block;
- `world_frame` optional → feature-by-presence (absent = index-space).

**One open sub-fork (needs explicit confirm before Accept):** §6 picks **LPS canonical** (DICOM-native → common path is a no-op) + normalise RAS at the door, with `convention` always recorded so affines are never ambiguous. Flag if you'd rather keep source-convention-as-stored.

---

# [Comment #2]() by [gerchowl]()

_Posted on June 26, 2026 at 03:30 PM_

§6 **locked: LPS canonical** (user-confirmed 2026-06-26). RAS normalised at the door (lossless sign-flip), `convention` retained for self-describing affines. All ADR-0030 decision points settled; promotes to Accepted with the 0026–0029 empirical-overhead spike.

---

# [Comment #3]() by [gerchowl]()

_Posted on September 28, 2026 at 05:54 PM_

Closing as **done** — verified on `origin/dev` in the 2026-09-28 backlog triage.

Evidence: ADR-0030 accepted (docs/adr/0030-spatial-referencing.md status: Accepted, as-built — §1–§7 all built + tested per fresh-context re-audit). world_frame wiring for DICOM series landed fdb457e (#271).

https://claude.ai/code/session_01XdERKMVDAwfMJSKdTytNnK

