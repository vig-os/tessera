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
# NIfTI's real choice is the container, not a codec knob: plain `.nii` is the uncompressed default
# every tool writes, `.nii.gz` is the ubiquitous compressed form. Those are the two settings a
# reader actually picks between, so they are the two variants (#485).
VARIANTS = {
    "default": "uncompressed (.nii)",
    "tuned": "gzip (.nii.gz)",
}
CAPS = {"volume": True, "table": False, "swmr": False}


def path_for(base: str, modality: str, variant: str = "tuned") -> str:
    return base + (".nii" if variant == "default" else ".nii.gz")


# ---- volume ----
def write_volume(base: str, vol: np.ndarray, variant: str = "tuned") -> None:
    # np.eye(4) affine → no axis reordering on read; data is stored exactly as (D, H, W).
    img = nib.Nifti1Image(vol, affine=np.eye(4))
    hdr = img.header
    hdr.set_data_dtype(vol.dtype)  # pin int16 on-disk, no implicit promotion
    hdr.set_slope_inter(1, 0)  # disable scl_slope/scl_inter scaling → reads stay int16
    nib.save(img, path_for(base, "volume", variant))


def read_volume(base: str, variant: str = "tuned") -> np.ndarray:
    img = nib.load(path_for(base, "volume", variant))
    # `np.array`, NOT `np.asarray` — and the difference is a fairness bug, not a style choice.
    #
    # For an UNCOMPRESSED `.nii`, nibabel's ArrayProxy is memory-mapped, so `np.asarray` returns a
    # view whose `.base` is a `np.memmap`: no bytes are read and no decode happens until something
    # touches the pages. It timed at 96,407 MB/s warm and 63,342 MB/s cold — not measurements, just
    # the cost of constructing a mapping. Every other adapter here materialises a real in-memory
    # array, so NIfTI was being timed doing strictly less work, and its COLD row was meaningless
    # because faulting would have happened later, on access, outside the timed region.
    #
    # `np.array` forces the copy, which is the contract (`read_volume -> ndarray, full`). NIfTI's
    # mmap capability is real and an advantage in practice; it is simply not what this row measures,
    # and crediting it here would let one format win by returning something cheaper.
    # `dataobj` (not `get_fdata`) still preserves the on-disk dtype — get_fdata would force float64.
    return np.array(img.dataobj)


def read_volume_zslice(base: str, z: int, variant: str = "tuned") -> np.ndarray:
    img = nib.load(path_for(base, "volume", variant))
    # Lazy slice through the ArrayProxy — only the gzip blocks covering plane z are decoded.
    # `np.array` for the same reason as above: the slice of a memmap-backed proxy is itself a view.
    return np.array(img.dataobj[z])


# ---- table (unsupported by NIfTI) ----
def write_table(base: str, cols: dict, variant: str = "tuned") -> None:
    raise NotImplementedError("NIfTI has no native table modality")


def read_table(base: str, variant: str = "tuned") -> dict:
    raise NotImplementedError("NIfTI has no native table modality")


def read_table_column(base: str, name: str, variant: str = "tuned") -> np.ndarray:
    raise NotImplementedError("NIfTI has no native table modality")
