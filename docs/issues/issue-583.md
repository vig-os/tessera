---
type: issue
state: open
created: 2026-10-16T23:40:04Z
updated: 2026-10-16T23:40:04Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/583
comments: 0
labels: feature
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-17T06:32:40.686Z
---

# [Issue 583]: [feat(py): reader-side export adapters — to_sitk / to_nifti / to_ome_zarr_store / to_xarray / to_dicom_seg / to_monai_dataset](https://github.com/vig-os/tessera/issues/583)

### Context

anatomist runs on data that leaves and re-enters tessera through established tools: Slicer and nnInteractive read NIfTI; ANTs/SimpleITK operate on ITK images and transforms; napari/MONAI read OME-Zarr; PACS/clinical viewers consume DICOM SEG; phase-9 distillation trains PyTorch/MONAI models. Today each of these means hand-rolled glue in anatomist code. Any FAIR-data producer hits the same wall, so the right home for these adapters is tessera, not downstream.

### Proposed Python surface (consumer-agnostic, lives in `tessera-py`)

```python
r = tessera.open("dog_07_recon.tsra")
r.to_sitk("volume")                 # -> SimpleITK.Image (origin/spacing/direction from world_frame)
r.to_nifti("volume", "out.nii.gz")  # export (sform from world_frame)
r.to_ome_zarr_store("volume")       # -> zarr.storage.Store (lazy, chunked, no extraction)
r.to_xarray("volume")               # -> xarray.DataArray on the store above
r.to_dicom_seg("roi", template="recon.tsra", out="seg.dcm")
r.to_monai_dataset(["volume","roi"]) # -> monai.data.Dataset, lazy ROI reads
```

### Why each matters for anatomist

- **NIfTI / SimpleITK export** — Slicer/nnInteractive input and ANTs template construction. Already have NIfTI ingest; the symmetric export is the missing half.
- **OME-Zarr store adapter** — the array block already IS a serialised Zarr v3 store (SPEC §5a). If tessera exposes it as a `zarr.storage.Store`, zarr-python v3 / tensorstore / xarray / dask read tessera lazily with zero extraction. Napari then opens a `.tsra` directly. This is the single highest-leverage adapter.
- **DICOM SEG export** — send segmentations back to PACS; interop with clinical viewers.
- **MONAI Dataset** — phase 9 distillation trains an inventory-conditioned CNN on tessera outputs. Reference: `monai.data.Dataset` / `CacheDataset` / `PersistentDataset`.

### Reference-format alignment

Each adapter is a one-to-one map onto an existing reference format: NIfTI-1 (sform/qform), ITK image (origin/spacing/direction), OME-NGFF 0.4+ (coordinateTransformations), DICOM SEG IOD, MONAI's dataset interface. tessera **matches** these formats semantically (ADR-0030, ADR-0032); only the glue is missing.

### Not asking for

- any format conversion that would **drop** tessera's seal/provenance (both export paths must emit the FAIR manifest alongside the derived file).
- a full OME-NGFF store rewrite — the `.tsra` zip STORED layout plus the serialised-store framing already contain everything a zarr-python Store needs.

