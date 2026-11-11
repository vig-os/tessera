---
type: issue
state: open
created: 2026-11-11T01:35:26Z
updated: 2026-11-11T01:35:35Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/825
comments: 0
labels: feature, priority:medium, area:ingest
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-11T08:50:30.277Z
---

# [Issue 825]: [feat(ingest): vendor ingest gaps from real GE/Siemens data — plain int16 BLF, per-slice RescaleSlope bit-exact, uint16 PET, streaming large npy](https://github.com/vig-os/tessera/issues/825)

## Description

Four ingest gaps hit while converting real GE Discovery MI Gen2 and Siemens Biograph Vision acquisitions (tessera `6a3078079`). Each one forced a workaround that was lossy or ran outside tessera.

- [ ] **GE list-mode BLF is rejected.** The error is `invalid product: ge-hdf5: dataset datatype is not a compound (got int16)`. `/MiceList/TofCoinc` is a plain N × 5 int16 dataset behind GE's closed HDF5 filter 32783 (GLEPL_MI). The only HDF5 path, `hdf-compound`, wants a compound type (`crates/tessera-ingest/src/ge_hdf5.rs:48-53`). Workaround used: decode with GE's plugin into compound or columns first. Proposal: accept plain N × k numeric datasets with a column-name map, and decode through a user-installed plugin (`HDF5_PLUGIN_PATH`) without shipping it.
- [ ] **PET DICOM with per-slice RescaleSlope is rejected bit-exact.** This holds for GE and Siemens alike. `dicom-series --rescale-mode bit-exact` refuses; `global-int16` re-quantises (lossy). Proposal: store the native integer pixels plus a per-slice slope/intercept vector ("store, don't compute", #219, #300).
- [ ] **Siemens PET uint16 pixels are decoded to i16 and fail** (`crates/tessera-ingest/src/dicom.rs:196-200`). Proposal: keep the native dtype (#192 direction).
- [ ] **The CLI runs out of memory on a 20 GB `.npy` array.** The array was a GE sinogram, uint16 (272, 415, 31, 2863), from the closed OBRIVN codec. It went in only via a streamed 272-block script. Proposal: streaming array ingest (memory-mapped `.npy`, one block per leading index).

## Fixtures

- DICOM: public TCIA ACRIN-NSCLC-FDG-PET (DOI 10.7937/tcia.2019.30ilqfcl). Its PET series carry per-slice RescaleSlope (GE) or uint16 (Siemens).
- GE list-mode BLF and sinogram: only on the source host. The BLF needs GE's licensed plugin and can't be shared. The decoded list-mode events are in the internal fixture set `tessera-vendor-2026-11` (see #822; ask the maintainer).

## Related

#208 (vendor raw ingest epic), #305 (GE list-mode HDF5 monolith as BLF-derived products), #753 (NIfTI uint8 sealed as uint16).

## Changelog Category

Added

