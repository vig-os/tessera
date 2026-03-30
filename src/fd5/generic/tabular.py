"""fd5.generic.tabular — Tabular product schema for spreadsheet-like data.

Implements the ``tabular`` product schema for lab measurements, clinical
records, parameter tables, and similar column-oriented data.
"""

from __future__ import annotations

from typing import Any

import h5py
import numpy as np

_SCHEMA_VERSION = "1.0.0"

_GZIP_LEVEL = 4

_ID_INPUTS = ["product", "name", "timestamp"]


class TabularSchema:
    """Product schema for column-oriented tabular data (``tabular``)."""

    product_type: str = "tabular"
    schema_version: str = _SCHEMA_VERSION

    def json_schema(self) -> dict[str, Any]:
        return {
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "type": "object",
            "properties": {
                "_schema_version": {"type": "integer"},
                "product": {"type": "string", "const": "tabular"},
                "name": {"type": "string"},
                "description": {"type": "string"},
                "column_count": {"type": "integer"},
                "row_count": {"type": "integer"},
            },
            "required": ["_schema_version", "product", "name", "description"],
        }

    def required_root_attrs(self) -> dict[str, Any]:
        return {"product": "tabular"}

    def id_inputs(self) -> list[str]:
        return list(_ID_INPUTS)

    def write(self, target: h5py.File | h5py.Group, data: dict[str, Any]) -> None:
        """Write tabular data to *target*.

        *data* must contain:
        - ``columns``: dict mapping column_name to numpy array (all same length)
        - ``description``: str

        Optional keys:
        - ``column_metadata``: dict mapping column_name to
          ``{"units": str, "description": str}``
        - ``row_labels``: numpy array or list of str (row identifiers)
        """
        columns = data["columns"]
        column_metadata = data.get("column_metadata", {})

        row_count = len(next(iter(columns.values())))
        target.attrs["column_count"] = np.int64(len(columns))
        target.attrs["row_count"] = np.int64(row_count)

        self._write_table(target, columns, column_metadata)

        if "row_labels" in data:
            self._write_row_labels(target, data["row_labels"])

    # ------------------------------------------------------------------
    # Table
    # ------------------------------------------------------------------

    def _write_table(
        self,
        target: h5py.File | h5py.Group,
        columns: dict[str, Any],
        column_metadata: dict[str, dict[str, str]],
    ) -> None:
        table_grp = target.create_group("table")
        for col_name, arr in columns.items():
            arr = np.asarray(arr)
            ds = table_grp.create_dataset(
                col_name,
                data=arr,
                compression="gzip",
                compression_opts=_GZIP_LEVEL,
            )
            meta = column_metadata.get(col_name, {})
            if "units" in meta:
                ds.attrs["units"] = meta["units"]
            if "description" in meta:
                ds.attrs["description"] = meta["description"]

    # ------------------------------------------------------------------
    # Row labels
    # ------------------------------------------------------------------

    def _write_row_labels(
        self,
        target: h5py.File | h5py.Group,
        row_labels: Any,
    ) -> None:
        encoded = [s.encode("utf-8") if isinstance(s, str) else s for s in row_labels]
        max_len = max(len(b) for b in encoded) if encoded else 1
        labels_arr = np.array(encoded, dtype=f"S{max_len}")
        target.create_dataset("row_labels", data=labels_arr)
