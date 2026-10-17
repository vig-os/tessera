---
type: issue
state: open
created: 2026-10-16T23:40:02Z
updated: 2026-10-16T23:40:02Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/582
comments: 0
labels: feature
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-17T06:32:41.044Z
---

# [Issue 582]: [feat(schema): grow transform beyond rigid+affine+displacement — typed models, composition, uncertainty, articulated/factor-graph, landmark sets](https://github.com/vig-os/tessera/issues/582)

### Context

ADR-0030 covers the **voxel→world affine on one array** and ADR-0032 generalises it to a `(transform, unit, frame)` descriptor on an **axis or value** of that array. For anatomist's uses this covers:

- CT/PET `recon` with LPS + mm (good)
- the time axis of `dynamic_pet`, decay-correction reference (good)
- `deformation_field` as a per-voxel displacement array with its own frame (good — ADR-0030 §5)
- rigid registration = "transform product + new frame + provenance edge" (good — FEATURE-MATRIX row)

What is **not** covered, from an anatomist read of ADR-0030 §5 + ADR-0032 §2 + the `transform` schema:

### Gaps for transforms *between* frames

1. **Typed transform-model library.** The `transform` product schema's block is a bare "affine (table) or deformation field (array)". There is no codified taxonomy of transform *kinds* with their canonical parameter layouts: `rigid3D` (6 DoF: tx,ty,tz, rx,ry,rz), `similarity3D` (7), `affine3D` (12, already in `affine_nd`), `BSpline` (grid + control points), `thin_plate_spline` (landmark pairs), `composite` (ordered list). ITK Transform HDF5 and ANTs ship these natively; OME-NGFF 0.5+ `coordinateTransformations` has `identity | translation | scale | rotation | affine | sequence | inverseOf | byDimension | coordinates | displacements`. Tessera's `affine_nd` matches `affine` only. The gap costs a reader that must apply ANTs-produced transforms with no way to tell by schema alone that bytes are a BSpline grid.

2. **Composite / chain transforms.** Anatomist registers dog→atlas as `T_affine ∘ T_warp` (ANTs). Today a consumer composes outside, with no sealed "the composition I used was these two products in this order". OME-NGFF `sequence` is the reference.

3. **Per-transform uncertainty.** A rigid registration carries a 6×6 Σ (SE(3) covariance); a factor-graph pose graph (frame A's articulated skull/limb frames, ADR §4 A) produces marginal 6×6 per segment, plus per-landmark 3×3 Σ. Nothing in the current `transform` schema accepts covariances. The right convention is a `uncertainty` block alongside the parameter table/array.

4. **Articulated / factor-graph transforms.** Anatomist's `A` has one skull frame + one frame per limb segment, linked by joint constraints. The right primitive is `pose_graph` (nodes: named frames with parameters; edges: joint constraints with factor types). Reference: GTSAM factor graphs, URDF-ish link/joint declarations.

5. **Named cross-product transform references.** A reader cannot ask "give me the transform between product-A's frame and product-B's frame" by name; it has to walk `derived_from` edges and know by convention which edge is the transform. OME-NGFF's `coordinateSystems` with named inputs/outputs is the reference.

6. **Landmarks as a transform input/output.** Anatomist tracks ~10 landmarks per dog with per-point Σ 3×3. There is no landmark schema. Reference: Slicer `.fcsv`, ANTs `.csv`, OME-NGFF `points` + `coordinateTransformations`.

### Proposal

Grow the existing `transform` schema from "array or table" to a typed family:

```
transform/kind ∈ { rigid3D, similarity3D, affine3D, bspline3D, tps3D, composite, pose_graph, displacement_field }
transform/parameters: block (table or array per kind)
transform/uncertainty: block (optional table or array)
transform/from_frame, transform/to_frame: strings matching CANONICAL_FRAMES or atlas:<id>
transform/components (composite only): ordered [transform_ref]
```

Add a `landmark_set` schema (table: `name, x,y,z, cov_6, source`).

### Reference format match

OME-NGFF `coordinateTransformations` v0.5+ and ITK Transform HDF5 together cover almost all of (1)–(5). ITK/ANTs is the compatibility bar for medical imaging; OME-NGFF is the bar for lazy multiscale readers. tessera **lags** both today for anything beyond rigid+affine+displacement.

