"""fd5.generic.ndarray — N-dimensional array product schema.

Implements the ``ndarray`` product schema for arbitrary N-dimensional arrays:
sensor grids, image stacks, simulation output, and similar dense numeric data.
"""

from __future__ import annotations

from typing import Any

import h5py
import numpy as np

_SCHEMA_VERSION = "1.0.0"

_GZIP_LEVEL = 4

_ID_INPUTS = ["product", "name", "timestamp"]


class NdarraySchema:
    """Product schema for arbitrary N-dimensional arrays (``ndarray``)."""

    product_type: str = "ndarray"
    schema_version: str = _SCHEMA_VERSION

    def json_schema(self) -> dict[str, Any]:
        return {
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "type": "object",
            "properties": {
                "_schema_version": {"type": "integer"},
                "product": {"type": "string", "const": "ndarray"},
                "name": {"type": "string"},
                "description": {"type": "string"},
                "dimension_order": {"type": "string"},
                "reference_frame": {"type": "string"},
            },
            "required": ["_schema_version", "product", "name", "description"],
        }

    def required_root_attrs(self) -> dict[str, Any]:
        return {"product": "ndarray"}

    def id_inputs(self) -> list[str]:
        return list(_ID_INPUTS)

    def write(self, target: h5py.File | h5py.Group, data: dict[str, Any]) -> None:
        """Write N-dimensional array data to *target*.

        *data* must contain:
        - ``array``: numpy ndarray (any shape, any numeric dtype)
        - ``dimension_order``: str (e.g. "TZYX", "XY", "ChannelHeightWidth")
        - ``description``: str

        Optional keys:
        - ``affine``: (4,4) float64 array
        - ``reference_frame``: str (e.g. "LPS", "RAS")
        - ``dimensions``: dict mapping axis_name to
          ``{"label": str, "units": str, "spacing": float}``
        """
        arr = np.asarray(data["array"])
        target.create_dataset(
            "array",
            data=arr,
            compression="gzip",
            compression_opts=_GZIP_LEVEL,
        )

        target.attrs["dimension_order"] = data["dimension_order"]

        if "reference_frame" in data:
            target.attrs["reference_frame"] = data["reference_frame"]

        if "affine" in data:
            target.create_dataset(
                "affine",
                data=np.asarray(data["affine"], dtype=np.float64),
            )

        if "dimensions" in data:
            self._write_dimensions(target, data["dimensions"])

    # ------------------------------------------------------------------
    # Dimensions
    # ------------------------------------------------------------------

    def _write_dimensions(
        self,
        target: h5py.File | h5py.Group,
        dimensions: dict[str, dict[str, Any]],
    ) -> None:
        dims_grp = target.create_group("dimensions")
        for axis_name, dim_info in dimensions.items():
            ax_grp = dims_grp.create_group(axis_name)
            if "label" in dim_info:
                ax_grp.attrs["label"] = dim_info["label"]
            if "units" in dim_info:
                ax_grp.attrs["units"] = dim_info["units"]
            if "spacing" in dim_info:
                ax_grp.attrs["spacing"] = np.float64(dim_info["spacing"])
