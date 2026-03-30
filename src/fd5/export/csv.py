"""fd5.export.csv — Export tabular/spectrum/timeseries data to CSV.

Reads product data from an fd5 file and writes a standard CSV file.
Supports product types: spectrum, device_data, and generic tabular data.
"""

from __future__ import annotations

import csv as csv_mod
from pathlib import Path

import h5py
import numpy as np


def extract_columns(
    fd5_path: str | Path,
    *,
    group: str | None = None,
) -> dict[str, np.ndarray]:
    """Read tabular column data from an fd5 file.

    Shared by :func:`export_csv` and :func:`~fd5.export.parquet.export_parquet`.
    """
    fd5_path = Path(fd5_path)
    with h5py.File(fd5_path, "r") as f:
        if group is not None:
            return _extract_group(f, group)
        product = _read_product(f)
        return _PRODUCT_EXTRACTORS.get(product, _extract_generic)(f)


def export_csv(
    fd5_path: str | Path,
    output_path: str | Path,
    *,
    group: str | None = None,
) -> Path:
    """Export tabular data from an fd5 file to CSV.

    Parameters
    ----------
    fd5_path:
        Path to the source fd5 (``.h5``) file.
    output_path:
        Destination path for the CSV file.
    group:
        Optional HDF5 group path to export from. If *None*, the product
        type is auto-detected from root attrs.

    Returns
    -------
    Path to the written CSV file.
    """
    output_path = Path(output_path)
    columns = extract_columns(fd5_path, group=group)
    _write_csv(output_path, columns)
    return output_path


# ---------------------------------------------------------------------------
# Product-type detection
# ---------------------------------------------------------------------------


def _read_product(f: h5py.File) -> str:
    """Read the product root attribute."""
    val = f.attrs.get("product", "")
    if isinstance(val, bytes):
        val = val.decode("utf-8")
    return val


# ---------------------------------------------------------------------------
# Data extraction per product type
# ---------------------------------------------------------------------------


def _extract_spectrum(f: h5py.File) -> dict[str, np.ndarray]:
    """Extract spectrum data: bin_centers + counts (+ counts_errors)."""
    columns: dict[str, np.ndarray] = {}

    if "counts" in f:
        counts = f["counts"][()]
        columns["counts"] = counts.ravel()

    if "counts_errors" in f:
        columns["counts_errors"] = f["counts_errors"][()].ravel()

    # Extract bin_centers from the first axis
    if "axes" in f:
        axes_grp = f["axes"]
        for ax_name in sorted(axes_grp.keys()):
            ax = axes_grp[ax_name]
            if "bin_centers" in ax:
                label = ax.attrs.get("label", ax_name)
                if isinstance(label, bytes):
                    label = label.decode("utf-8")
                columns[label] = ax["bin_centers"][()]

    # Reorder so axis columns come first
    reordered: dict[str, np.ndarray] = {}
    for key in columns:
        if key not in ("counts", "counts_errors"):
            reordered[key] = columns[key]
    for key in ("counts", "counts_errors"):
        if key in columns:
            reordered[key] = columns[key]

    return reordered


def _extract_device_data(f: h5py.File) -> dict[str, np.ndarray]:
    """Extract device_data: time + signal per channel."""
    columns: dict[str, np.ndarray] = {}

    if "channels" not in f:
        return columns

    channels_grp = f["channels"]
    time_written = False

    for ch_name in sorted(channels_grp.keys()):
        ch = channels_grp[ch_name]

        # Write time column from the first channel only
        if not time_written and "time" in ch:
            columns["time"] = ch["time"][()]
            time_written = True

        if "signal" in ch:
            columns[ch_name] = ch["signal"][()]

    return columns


def _extract_1d_datasets(group: h5py.Group) -> dict[str, np.ndarray]:
    """Extract all 1D datasets from an HDF5 group."""
    columns: dict[str, np.ndarray] = {}
    for key in sorted(group.keys()):
        item = group[key]
        if isinstance(item, h5py.Dataset) and item.ndim == 1:
            columns[key] = item[()]
    return columns


def _extract_generic(f: h5py.File) -> dict[str, np.ndarray]:
    """Fallback: extract all 1D datasets from root level."""
    return _extract_1d_datasets(f)


def _extract_group(f: h5py.File, group: str) -> dict[str, np.ndarray]:
    """Extract all 1D datasets from a specific group."""
    if group not in f:
        raise KeyError(f"Group {group!r} not found in file")
    return _extract_1d_datasets(f[group])


_PRODUCT_EXTRACTORS = {
    "spectrum": _extract_spectrum,
    "device_data": _extract_device_data,
}


# ---------------------------------------------------------------------------
# CSV writer
# ---------------------------------------------------------------------------


def _write_csv(output_path: Path, columns: dict[str, np.ndarray]) -> None:
    """Write column dict to CSV file."""
    if not columns:
        raise ValueError("No tabular data found to export")

    headers = list(columns.keys())
    arrays = [columns[h] for h in headers]
    n_rows = max(len(a) for a in arrays)

    output_path.parent.mkdir(parents=True, exist_ok=True)

    with output_path.open("w", newline="") as fh:
        writer = csv_mod.writer(fh)
        writer.writerow(headers)
        for i in range(n_rows):
            row = [str(a[i]) if i < len(a) else "" for a in arrays]
            writer.writerow(row)
