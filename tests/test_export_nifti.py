"""Tests for fd5.export.nifti — NIfTI export from fd5 recon files."""

from __future__ import annotations

from pathlib import Path

import h5py
import nibabel as nib
import numpy as np
import pytest

from fd5.export.nifti import export_nifti


# ---------------------------------------------------------------------------
# Fixtures
# ---------------------------------------------------------------------------


@pytest.fixture()
def recon_fd5_3d(tmp_path: Path) -> Path:
    """Create a minimal fd5 file with a 3D volume (ZYX order)."""
    path = tmp_path / "recon_3d.h5"
    vol = np.arange(24, dtype=np.float32).reshape(2, 3, 4)
    affine = np.diag([2.0, 2.0, 2.0, 1.0])
    with h5py.File(path, "w") as f:
        f.attrs["product"] = "recon"
        f.attrs["dimension_order"] = "ZYX"
        f.create_dataset("volume", data=vol)
        f.create_dataset("affine", data=affine)
    return path


@pytest.fixture()
def recon_fd5_4d(tmp_path: Path) -> Path:
    """Create a minimal fd5 file with a 4D volume (TZYX order)."""
    path = tmp_path / "recon_4d.h5"
    vol = np.arange(48, dtype=np.float32).reshape(2, 2, 3, 4)
    affine = np.eye(4)
    with h5py.File(path, "w") as f:
        f.attrs["product"] = "recon"
        f.attrs["dimension_order"] = "TZYX"
        f.create_dataset("volume", data=vol)
        f.create_dataset("affine", data=affine)
    return path


@pytest.fixture()
def recon_fd5_no_affine(tmp_path: Path) -> Path:
    """Create a minimal fd5 file with no affine (should default to eye(4))."""
    path = tmp_path / "recon_no_affine.h5"
    vol = np.ones((3, 4, 5), dtype=np.float32)
    with h5py.File(path, "w") as f:
        f.attrs["product"] = "recon"
        f.create_dataset("volume", data=vol)
    return path


# ---------------------------------------------------------------------------
# Tests
# ---------------------------------------------------------------------------


def test_export_3d_roundtrip(recon_fd5_3d: Path, tmp_path: Path) -> None:
    """Exported 3D NIfTI should contain the same data and affine."""
    out = tmp_path / "output.nii.gz"
    result = export_nifti(recon_fd5_3d, out)

    assert result == out
    assert out.exists()

    img = nib.load(out)
    data = np.asarray(img.dataobj)
    # Original ZYX (2,3,4) reordered to XYZ (4,3,2)
    assert data.shape == (4, 3, 2)

    # Verify affine preserved
    np.testing.assert_allclose(img.affine, np.diag([2.0, 2.0, 2.0, 1.0]))


def test_export_4d_roundtrip(recon_fd5_4d: Path, tmp_path: Path) -> None:
    """Exported 4D NIfTI should reorder TZYX to TXYZ."""
    out = tmp_path / "output_4d.nii.gz"
    result = export_nifti(recon_fd5_4d, out)

    assert result == out
    img = nib.load(out)
    data = np.asarray(img.dataobj)
    # TZYX (2,2,3,4) -> TXYZ (2,4,3,2)
    assert data.shape == (2, 4, 3, 2)


def test_export_missing_affine_defaults_to_identity(
    recon_fd5_no_affine: Path, tmp_path: Path
) -> None:
    """When no affine is stored, default to np.eye(4)."""
    out = tmp_path / "output_no_affine.nii.gz"
    export_nifti(recon_fd5_no_affine, out)

    img = nib.load(out)
    np.testing.assert_allclose(img.affine, np.eye(4))


def test_export_missing_dataset_raises(recon_fd5_3d: Path, tmp_path: Path) -> None:
    """Requesting a non-existent dataset should raise KeyError."""
    out = tmp_path / "output.nii.gz"
    with pytest.raises(KeyError, match="nonexistent"):
        export_nifti(recon_fd5_3d, out, dataset="nonexistent")


def test_export_custom_dataset(tmp_path: Path) -> None:
    """Export from a non-default dataset path."""
    fd5_path = tmp_path / "custom.h5"
    vol = np.ones((3, 4, 5), dtype=np.float32) * 42.0
    with h5py.File(fd5_path, "w") as f:
        f.attrs["product"] = "recon"
        f.attrs["dimension_order"] = "ZYX"
        f.create_dataset("my_volume", data=vol)

    out = tmp_path / "output.nii.gz"
    export_nifti(fd5_path, out, dataset="my_volume")

    img = nib.load(out)
    data = np.asarray(img.dataobj)
    assert data.shape == (5, 4, 3)
    np.testing.assert_allclose(data, 42.0)
