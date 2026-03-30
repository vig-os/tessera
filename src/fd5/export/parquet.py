"""fd5.export.parquet — Export tabular/spectrum/timeseries data to Parquet.

Same data extraction as the CSV exporter but writes via pyarrow.
Preserves column dtypes (float64, int64, string).
Requires pyarrow (optional dependency).
"""

from __future__ import annotations

from pathlib import Path

from fd5.export.csv import extract_columns


def export_parquet(
    fd5_path: str | Path,
    output_path: str | Path,
    *,
    group: str | None = None,
) -> Path:
    """Export tabular data from an fd5 file to Apache Parquet.

    Parameters
    ----------
    fd5_path:
        Path to the source fd5 (``.h5``) file.
    output_path:
        Destination path for the Parquet file.
    group:
        Optional HDF5 group path to export from. If *None*, the product
        type is auto-detected from root attrs.

    Returns
    -------
    Path to the written Parquet file.
    """
    try:
        import pyarrow as pa
        import pyarrow.parquet as pq
    except ImportError:
        raise ImportError(
            "pyarrow is required for Parquet export. "
            "Install it with: pip install 'fd5[parquet]'"
        ) from None

    output_path = Path(output_path)
    columns = extract_columns(fd5_path, group=group)

    if not columns:
        raise ValueError("No tabular data found to export")

    table = pa.table(dict(columns))

    output_path.parent.mkdir(parents=True, exist_ok=True)
    pq.write_table(table, output_path)
    return output_path
