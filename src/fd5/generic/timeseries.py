"""fd5.generic.timeseries — Timeseries product schema for continuous sensor data.

Implements the ``timeseries`` product schema for continuous sensor data,
physiological monitoring, IoT streams, and similar time-domain signals.
"""

from __future__ import annotations

from typing import Any

import h5py
import numpy as np

_SCHEMA_VERSION = "1.0.0"

_GZIP_LEVEL = 4

_ID_INPUTS = ["product", "name", "timestamp"]


class TimeseriesSchema:
    """Product schema for continuous time-series data (``timeseries``)."""

    product_type: str = "timeseries"
    schema_version: str = _SCHEMA_VERSION

    def json_schema(self) -> dict[str, Any]:
        return {
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "type": "object",
            "properties": {
                "_schema_version": {"type": "integer"},
                "product": {"type": "string", "const": "timeseries"},
                "name": {"type": "string"},
                "description": {"type": "string"},
                "sampling_rate": {"type": "number"},
                "sampling_rate_units": {"type": "string", "const": "Hz"},
            },
            "required": ["_schema_version", "product", "name", "description"],
        }

    def required_root_attrs(self) -> dict[str, Any]:
        return {"product": "timeseries"}

    def id_inputs(self) -> list[str]:
        return list(_ID_INPUTS)

    def write(self, target: h5py.File | h5py.Group, data: dict[str, Any]) -> None:
        """Write timeseries data to *target*.

        *data* must contain:
        - ``signals``: dict mapping channel_name to numpy 1D float array
        - ``time``: numpy 1D float array (timestamps in seconds)
        - ``description``: str

        Optional keys:
        - ``sampling_rate``: float (Hz)
        - ``events``: dict with ``timestamps`` (array) and ``labels`` (list[str])
        - ``metadata``: dict of additional attrs
        """
        self._write_signals(target, data["signals"], data.get("description", ""))
        self._write_time(target, data["time"])

        if "sampling_rate" in data:
            target.attrs["sampling_rate"] = np.float64(data["sampling_rate"])
            target.attrs["sampling_rate_units"] = "Hz"

        if "events" in data:
            self._write_events(target, data["events"])

        if "metadata" in data:
            self._write_metadata(target, data["metadata"])

    # ------------------------------------------------------------------
    # Signals
    # ------------------------------------------------------------------

    def _write_signals(
        self,
        target: h5py.File | h5py.Group,
        signals: dict[str, Any],
        description: str,
    ) -> None:
        signals_grp = target.create_group("signals")
        for name, arr in signals.items():
            ds = signals_grp.create_dataset(
                name,
                data=np.asarray(arr, dtype=np.float64),
                compression="gzip",
                compression_opts=_GZIP_LEVEL,
            )
            ds.attrs["units"] = "a.u."
            ds.attrs["description"] = description

    # ------------------------------------------------------------------
    # Time
    # ------------------------------------------------------------------

    def _write_time(
        self,
        target: h5py.File | h5py.Group,
        time: Any,
    ) -> None:
        ds = target.create_dataset(
            "time",
            data=np.asarray(time, dtype=np.float64),
            compression="gzip",
            compression_opts=_GZIP_LEVEL,
        )
        ds.attrs["units"] = "s"

    # ------------------------------------------------------------------
    # Events
    # ------------------------------------------------------------------

    def _write_events(
        self,
        target: h5py.File | h5py.Group,
        events: dict[str, Any],
    ) -> None:
        events_grp = target.create_group("events")
        events_grp.create_dataset(
            "timestamps",
            data=np.asarray(events["timestamps"], dtype=np.float64),
            compression="gzip",
            compression_opts=_GZIP_LEVEL,
        )
        encoded = [
            s.encode("utf-8") if isinstance(s, str) else s for s in events["labels"]
        ]
        max_len = max(len(b) for b in encoded) if encoded else 1
        labels_arr = np.array(encoded, dtype=f"S{max_len}")
        events_grp.create_dataset("labels", data=labels_arr)

    # ------------------------------------------------------------------
    # Metadata
    # ------------------------------------------------------------------

    def _write_metadata(
        self,
        target: h5py.File | h5py.Group,
        metadata: dict[str, Any],
    ) -> None:
        for key, value in metadata.items():
            if isinstance(value, float):
                target.attrs[key] = np.float64(value)
            elif isinstance(value, int):
                target.attrs[key] = np.int64(value)
            elif isinstance(value, str):
                target.attrs[key] = value
