"""Tests for fd5.export.parquet — Parquet export from fd5 files."""

from __future__ import annotations

from pathlib import Path

import h5py
import numpy as np
import pyarrow.parquet as pq
import pytest

from fd5.export.parquet import export_parquet


# ---------------------------------------------------------------------------
# Fixtures
# ---------------------------------------------------------------------------


@pytest.fixture()
def spectrum_fd5(tmp_path: Path) -> Path:
    """Create a minimal fd5 spectrum file."""
    path = tmp_path / "spectrum.h5"
    counts = np.array([10.0, 25.0, 18.0, 7.0], dtype=np.float32)
    bin_edges = np.array([50.0, 150.0, 250.0, 350.0, 450.0], dtype=np.float64)
    bin_centers = 0.5 * (bin_edges[:-1] + bin_edges[1:])

    with h5py.File(path, "w") as f:
        f.attrs["product"] = "spectrum"
        f.create_dataset("counts", data=counts)
        axes = f.create_group("axes")
        ax0 = axes.create_group("ax0")
        ax0.attrs["label"] = "energy"
        ax0.attrs["units"] = "keV"
        ax0.attrs["unitSI"] = 1.602e-16
        ax0.attrs["description"] = "Photon energy"
        ax0.create_dataset("bin_edges", data=bin_edges)
        ax0.create_dataset("bin_centers", data=bin_centers)
    return path


@pytest.fixture()
def device_data_fd5(tmp_path: Path) -> Path:
    """Create a minimal fd5 device_data file."""
    path = tmp_path / "device_data.h5"
    time = np.array([0.0, 1.0, 2.0], dtype=np.float64)
    temp = np.array([22.5, 22.6, 22.4], dtype=np.float64)

    with h5py.File(path, "w") as f:
        f.attrs["product"] = "device_data"
        channels = f.create_group("channels")
        ch = channels.create_group("temperature")
        ch.create_dataset("signal", data=temp)
        ch.create_dataset("time", data=time)
    return path


# ---------------------------------------------------------------------------
# Tests
# ---------------------------------------------------------------------------


def test_export_spectrum_parquet(spectrum_fd5: Path, tmp_path: Path) -> None:
    """Spectrum export to Parquet should preserve column types and values."""
    out = tmp_path / "spectrum.parquet"
    result = export_parquet(spectrum_fd5, out)

    assert result == out
    assert out.exists()

    table = pq.read_table(out)
    assert "energy" in table.column_names
    assert "counts" in table.column_names

    counts = table.column("counts").to_numpy()
    np.testing.assert_allclose(counts, [10.0, 25.0, 18.0, 7.0], atol=0.1)


def test_export_device_data_parquet(device_data_fd5: Path, tmp_path: Path) -> None:
    """Device data export to Parquet should include time + channels."""
    out = tmp_path / "device.parquet"
    export_parquet(device_data_fd5, out)

    table = pq.read_table(out)
    assert "time" in table.column_names
    assert "temperature" in table.column_names
    assert len(table) == 3


def test_export_empty_raises(tmp_path: Path) -> None:
    """Exporting an fd5 file with no tabular data should raise ValueError."""
    fd5_path = tmp_path / "empty.h5"
    with h5py.File(fd5_path, "w") as f:
        f.attrs["product"] = "unknown_product"

    out = tmp_path / "empty.parquet"
    with pytest.raises(ValueError, match="No tabular data"):
        export_parquet(fd5_path, out)


def test_export_with_group(tmp_path: Path) -> None:
    """Export from a specific group should work."""
    fd5_path = tmp_path / "grouped.h5"
    with h5py.File(fd5_path, "w") as f:
        grp = f.create_group("my_data")
        grp.create_dataset("x", data=np.array([1.0, 2.0, 3.0]))
        grp.create_dataset("y", data=np.array([4.0, 5.0, 6.0]))

    out = tmp_path / "grouped.parquet"
    export_parquet(fd5_path, out, group="my_data")

    table = pq.read_table(out)
    assert table.column_names == ["x", "y"]
    assert len(table) == 3
    np.testing.assert_allclose(table.column("x").to_numpy(), [1.0, 2.0, 3.0])


def test_export_preserves_float64_dtype(tmp_path: Path) -> None:
    """Parquet export should preserve float64 precision."""
    fd5_path = tmp_path / "precise.h5"
    vals = np.array([1.23456789012345, 9.87654321098765], dtype=np.float64)
    with h5py.File(fd5_path, "w") as f:
        f.attrs["product"] = "generic"
        f.create_dataset("values", data=vals)

    out = tmp_path / "precise.parquet"
    export_parquet(fd5_path, out)

    table = pq.read_table(out)
    np.testing.assert_array_equal(table.column("values").to_numpy(), vals)
