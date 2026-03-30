"""Tests for fd5.export.csv — CSV export from fd5 files."""

from __future__ import annotations

import csv as csv_mod
from pathlib import Path

import h5py
import numpy as np
import pytest

from fd5.export.csv import export_csv


# ---------------------------------------------------------------------------
# Fixtures
# ---------------------------------------------------------------------------


@pytest.fixture()
def spectrum_fd5(tmp_path: Path) -> Path:
    """Create a minimal fd5 spectrum file with counts + axes."""
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
    """Create a minimal fd5 device_data file with two channels."""
    path = tmp_path / "device_data.h5"
    time = np.array([0.0, 1.0, 2.0], dtype=np.float64)
    temp = np.array([22.5, 22.6, 22.4], dtype=np.float64)
    pressure = np.array([101.3, 101.2, 101.4], dtype=np.float64)

    with h5py.File(path, "w") as f:
        f.attrs["product"] = "device_data"
        channels = f.create_group("channels")
        ch_temp = channels.create_group("temperature")
        ch_temp.create_dataset("signal", data=temp)
        ch_temp.create_dataset("time", data=time)
        ch_press = channels.create_group("pressure")
        ch_press.create_dataset("signal", data=pressure)
        ch_press.create_dataset("time", data=time)
    return path


@pytest.fixture()
def empty_fd5(tmp_path: Path) -> Path:
    """Create an fd5 file with no exportable data."""
    path = tmp_path / "empty.h5"
    with h5py.File(path, "w") as f:
        f.attrs["product"] = "unknown_product"
    return path


# ---------------------------------------------------------------------------
# Tests
# ---------------------------------------------------------------------------


def test_export_spectrum_csv(spectrum_fd5: Path, tmp_path: Path) -> None:
    """Spectrum export should produce CSV with energy + counts columns."""
    out = tmp_path / "spectrum.csv"
    result = export_csv(spectrum_fd5, out)

    assert result == out
    assert out.exists()

    with out.open() as fh:
        reader = csv_mod.reader(fh)
        rows = list(reader)

    headers = rows[0]
    assert "energy" in headers
    assert "counts" in headers

    # energy column comes before counts
    assert headers.index("energy") < headers.index("counts")

    # Verify data values
    data_rows = rows[1:]
    assert len(data_rows) == 4
    counts_idx = headers.index("counts")
    assert float(data_rows[0][counts_idx]) == pytest.approx(10.0)


def test_export_spectrum_with_errors(tmp_path: Path) -> None:
    """Spectrum with counts_errors should include that column."""
    fd5_path = tmp_path / "spectrum_errors.h5"
    counts = np.array([10.0, 25.0], dtype=np.float32)
    errors = np.array([3.16, 5.0], dtype=np.float32)

    with h5py.File(fd5_path, "w") as f:
        f.attrs["product"] = "spectrum"
        f.create_dataset("counts", data=counts)
        f.create_dataset("counts_errors", data=errors)

    out = tmp_path / "out.csv"
    export_csv(fd5_path, out)

    with out.open() as fh:
        reader = csv_mod.reader(fh)
        rows = list(reader)

    assert "counts_errors" in rows[0]


def test_export_device_data_csv(device_data_fd5: Path, tmp_path: Path) -> None:
    """Device data export should produce CSV with time + channel columns."""
    out = tmp_path / "device.csv"
    result = export_csv(device_data_fd5, out)

    assert result == out
    assert out.exists()

    with out.open() as fh:
        reader = csv_mod.reader(fh)
        rows = list(reader)

    headers = rows[0]
    assert "time" in headers
    assert "pressure" in headers
    assert "temperature" in headers

    data_rows = rows[1:]
    assert len(data_rows) == 3


def test_export_empty_raises(empty_fd5: Path, tmp_path: Path) -> None:
    """Exporting an fd5 file with no tabular data should raise ValueError."""
    out = tmp_path / "empty.csv"
    with pytest.raises(ValueError, match="No tabular data"):
        export_csv(empty_fd5, out)


def test_export_with_group(tmp_path: Path) -> None:
    """Export from a specific group should work."""
    fd5_path = tmp_path / "grouped.h5"
    with h5py.File(fd5_path, "w") as f:
        grp = f.create_group("my_data")
        grp.create_dataset("x", data=np.array([1.0, 2.0, 3.0]))
        grp.create_dataset("y", data=np.array([4.0, 5.0, 6.0]))

    out = tmp_path / "grouped.csv"
    export_csv(fd5_path, out, group="my_data")

    with out.open() as fh:
        reader = csv_mod.reader(fh)
        rows = list(reader)

    assert rows[0] == ["x", "y"]
    assert len(rows) == 4  # header + 3 data rows


def test_export_missing_group_raises(tmp_path: Path) -> None:
    """Requesting a non-existent group should raise KeyError."""
    fd5_path = tmp_path / "test.h5"
    with h5py.File(fd5_path, "w") as f:
        f.attrs["product"] = "spectrum"

    out = tmp_path / "out.csv"
    with pytest.raises(KeyError, match="nonexistent"):
        export_csv(fd5_path, out, group="nonexistent")
