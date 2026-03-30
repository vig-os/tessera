"""fd5.export.nifti — Export recon/ndarray volumes to NIfTI format.

Reads volume data + spatial metadata from an fd5 file and writes a
NIfTI-1 ``.nii.gz`` file preserving the affine transform.
Requires nibabel (optional dependency).
"""

from __future__ import annotations

from pathlib import Path

import h5py
import numpy as np


def export_nifti(
    fd5_path: str | Path,
    output_path: str | Path,
    *,
    dataset: str = "volume",
) -> Path:
    """Export a volume dataset from an fd5 file to NIfTI format.

    Parameters
    ----------
    fd5_path:
        Path to the source fd5 (``.h5``) file.
    output_path:
        Destination path for the NIfTI file (``.nii`` or ``.nii.gz``).
    dataset:
        HDF5 dataset path containing the volume data (default ``"volume"``).

    Returns
    -------
    Path to the written NIfTI file.
    """
    try:
        import nibabel as nib
    except ImportError:
        raise ImportError(
            "nibabel is required for NIfTI export. "
            "Install it with: pip install 'fd5[nifti]'"
        ) from None

    fd5_path = Path(fd5_path)
    output_path = Path(output_path)

    with h5py.File(fd5_path, "r") as f:
        if dataset not in f:
            raise KeyError(f"Dataset {dataset!r} not found in {fd5_path}")

        data = f[dataset][()]

        affine = _read_affine(f)
        dim_order = _read_dimension_order(f)

    data = _reorder_to_nifti(data, dim_order)

    img = nib.Nifti1Image(data, affine)
    nib.save(img, output_path)
    return output_path


def _read_affine(f: h5py.File) -> np.ndarray:
    """Read affine from the fd5 file, falling back to identity."""
    if "affine" in f:
        return np.asarray(f["affine"][()], dtype=np.float64)
    if "affine" in f.attrs:
        return np.asarray(f.attrs["affine"], dtype=np.float64).reshape(4, 4)
    return np.eye(4, dtype=np.float64)


def _read_dimension_order(f: h5py.File) -> str:
    """Read dimension_order attribute, defaulting to ZYX."""
    for source in (f, f.get("volume")):
        if source is not None and "dimension_order" in getattr(source, "attrs", {}):
            val = source.attrs["dimension_order"]
            if isinstance(val, bytes):
                val = val.decode("utf-8")
            return val
    return "ZYX"


def _reorder_to_nifti(data: np.ndarray, dim_order: str) -> np.ndarray:
    """Reorder axes from fd5 dimension_order to NIfTI convention (XYZ[T]).

    fd5 stores volumes as ZYX (or TZYX for 4D). NIfTI expects the spatial
    axes in XYZ order (fastest axis first).
    """
    if dim_order in ("ZYX", "TZYX"):
        n_spatial = 3
        n_extra = data.ndim - n_spatial
        axes = list(range(n_extra)) + list(range(data.ndim - 1, n_extra - 1, -1))
        return np.transpose(data, axes)
    return data
