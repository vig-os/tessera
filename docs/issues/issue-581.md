---
type: issue
state: open
created: 2026-10-16T23:40:00Z
updated: 2026-10-16T23:40:00Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/581
comments: 0
labels: feature
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-17T06:32:41.368Z
---

# [Issue 581]: [feat(schema): richer segmentation semantics on roi — label registry, multi-channel probability, hierarchical labels, DICOM SEG export](https://github.com/vig-os/tessera/issues/581)

### Context

anatomist produces segmentations whose semantics tessera's current `roi` schema cannot express without ad-hoc `extra` fields. The pipeline is Bayesian, so a segmentation is:
- a **set of structures** with declared `parent` nesting (organ → lobe), each carrying `uberon`, `visibility`, `anchor`, `n_eff`, provenance (template v1);
- **per-voxel posterior probabilities** across structures (multi-channel float32), not just a single-label uint16;
- an **output state per structure** (`named_boundary_supported`, `candidate_ambiguous`, `outside_field_of_view`, `possibly_absent_requires_review`, …) that is clinically load-bearing.

### Current `roi` schema is thin

`roi` is "representation chosen by the ROI's nature" — a raster label array **or** a parametric/contour/stats table — but has no fields for a label registry, no multi-channel probability convention, no overlapping-label rules, no DICOM SEG export, no OME-NGFF `labels/` directory convention.

### Proposal (segmentation semantics on top of `roi`)

1. **Label registry** — a required `labels` table with columns `label_id, name, parent_label_id, uberon?, color_rgba?, visibility?, n_eff?, provenance?`. Lets a reader render colors, walk the hierarchy, and resolve "which organ is this lobe inside?" without a schema sidecar. Reference: OME-NGFF `labels/0/.zattrs`, DICOM-SEG `SegmentIdentificationSequence`, NIfTI label-tags (`.labels` sidecar).
2. **Multi-channel probability** — a reserved axis convention `axes = ["c", "z", "y", "x"]` plus a `channels` table that names each channel's `label_id`, mapping channel-axis entries to the label registry. This generalises `recon`'s single-channel array without inventing a new product kind. `channels` can also carry per-structure output state.
3. **Hierarchical / overlapping labels** — allow label stacks: an `_overlay` axis (sparse, dtype `uint32[]`) when voxels carry multiple labels (organ + lobe + lesion). Reference: OME-NGFF label hierarchies.
4. **DICOM SEG export** (optional) — `tessera export dicom-seg <roi.tsra>` so results can go back to a PACS. Reference: DICOM SEG IOD, dcmqi.

### Why in tessera, not anatomist

Any segmentation producer (nnInteractive, TotalSegmentator, user-drawn Slicer masks) hits the same shape. Tessera already carries affine + units; adding codified label/channel/state semantics closes the FAIR gap for segmentations.

