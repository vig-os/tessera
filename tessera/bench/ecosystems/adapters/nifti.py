"""NIfTI (nibabel) adapter for the cross-ecosystem I/O comparison (#143).

Volume-only — NIfTI has no native table modality, so the table funcs raise
NotImplementedError and the driver skips them per CAPS.

Bit-exact int16 round-trip notes:
- Build with `Nifti1Image(vol, np.eye(4))` so axes aren't reordered; the array is
  stored in the same (D, H, W) order it came in.
- Force the on-disk datatype to int16 and pin scl_slope=1 / scl_inter=0, otherwise
  nibabel can stamp an implicit scaling that turns reads into float64.
- Read back via `np.asarray(img.dataobj)` (NOT `get_fdata`, which always casts to
  float64) so dtype stays int16.
- Single-plane reads use the lazy `dataobj[z]` proxy so only that plane is
  decompressed (and only the gzip blocks covering it are read).
"""

from __future__ import annotations

import nibabel as nib
import numpy as np

NAME = "NIfTI (nibabel)"
CODEC = "gzip (.nii.gz)"
CAPS = {"volume": True, "table": False, "swmr": False}


def path_for(base: str, modality: str) -> str:
    return base + ".nii.gz"


# ---- volume ----
def write_volume(base: str, vol: np.ndarray) -> None:
    # np.eye(4) affine → no axis reordering on read; data is stored exactly as (D, H, W).
    img = nib.Nifti1Image(vol, affine=np.eye(4))
    hdr = img.header
    hdr.set_data_dtype(vol.dtype)          # pin int16 on-disk, no implicit promotion
    hdr.set_slope_inter(1, 0)              # disable scl_slope/scl_inter scaling → reads stay int16
    nib.save(img, path_for(base, "volume"))


def read_volume(base: str) -> np.ndarray:
    img = nib.load(path_for(base, "volume"))
    # asarray over dataobj preserves the on-disk dtype (int16); get_fdata would force float64.
    return np.asarray(img.dataobj)


def read_volume_zslice(base: str, z: int) -> np.ndarray:
    img = nib.load(path_for(base, "volume"))
    # Lazy slice through the ArrayProxy — only the gzip blocks covering plane z are decoded.
    return np.asarray(img.dataobj[z])


# ---- table (unsupported by NIfTI) ----
def write_table(base: str, cols: dict) -> None:
    raise NotImplementedError("NIfTI has no native table modality")


def read_table(base: str) -> dict:
    raise NotImplementedError("NIfTI has no native table modality")


def read_table_column(base: str, name: str) -> np.ndarray:
    raise NotImplementedError("NIfTI has no native table modality")
