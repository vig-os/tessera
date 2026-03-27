"""fd5.imaging.recon — Recon product schema for reconstructed image volumes.

Implements the ``recon`` product schema per white-paper.md § recon.
Handles 3D/4D/5D float32 volumes with multiscale pyramids, MIP projections,
dynamic frames, affine transforms, and chunked gzip compression.

Optional features (per white-paper.md § recon):
- ``mips_per_frame/``: per-frame coronal/sagittal MIPs for 4D+ data
- ``gate_phase`` and ``gate_trigger/`` sub-groups in ``frames/`` for gated recon
- ``device_data/``: embedded device streams (ECG, bellows) following NXlog pattern
- ``provenance/dicom_header``: JSON string from pydicom
- ``provenance/per_slice_metadata``: compound dataset with per-slice DICOM fields
"""

from __future__ import annotations

import json
from typing import Any

import h5py
import numpy as np

_SCHEMA_VERSION = "1.1.0"

_GZIP_LEVEL = 4

_ID_INPUTS = ["timestamp", "scanner", "vendor_series_id"]


class ReconSchema:
    """Product schema for reconstructed image volumes (``recon``)."""

    product_type: str = "recon"
    schema_version: str = _SCHEMA_VERSION

    def json_schema(self) -> dict[str, Any]:
        return {
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "type": "object",
            "properties": {
                "_schema_version": {"type": "integer"},
                "product": {"type": "string", "const": "recon"},
                "name": {"type": "string"},
                "description": {"type": "string"},
                "domain": {"type": "string"},
                "volume": {
                    "type": "object",
                    "description": "Root-level volume dataset (represented as attrs in h5_to_dict)",
                },
                "mips": {
                    "type": "object",
                    "description": "MIP projections (coronal, sagittal, axial); N-D for dynamic data",
                },
                "frames": {
                    "type": "object",
                    "description": "Frame timing, gating phase, and trigger data for 4D+ volumes",
                },
                "device_data": {
                    "type": "object",
                    "description": "Embedded device streams (ECG, bellows) following NXlog pattern",
                },
                "provenance": {
                    "type": "object",
                    "description": "Original file provenance, DICOM header, per-slice metadata",
                },
            },
            "required": ["_schema_version", "product", "name", "description"],
        }

    def required_root_attrs(self) -> dict[str, Any]:
        return {
            "product": "recon",
            "domain": "medical_imaging",
        }

    def id_inputs(self) -> list[str]:
        return list(_ID_INPUTS)

    def write(self, target: h5py.File | h5py.Group, data: dict[str, Any]) -> None:
        """Write recon data to *target*.

        *data* must contain:
        - ``volume``: numpy float32 array (3D, 4D, or 5D)
        - ``affine``: float64 (4, 4) spatial affine matrix
        - ``dimension_order``: str (e.g. ``"ZYX"``, ``"TZYX"``)
        - ``reference_frame``: str (e.g. ``"LPS"``)
        - ``description``: str describing the volume

        Optional keys:
        - ``frames``: dict with frame timing for 4D+ data
        - ``pyramid``: dict with ``scale_factors`` and ``method``
        - ``mips_per_frame``: bool — write per-frame MIPs (requires 4D+ volume)
        - ``device_data``: dict of channel dicts for embedded device signals
        - ``provenance``: dict with ``dicom_header`` and/or ``per_slice_metadata``
        """
        target.attrs["default"] = "volume"

        volume = data["volume"]
        self._write_volume(target, volume, data)

        spatial_vol = _spatial_volume(volume, data["dimension_order"])

        if "frames" in data:
            self._write_frames(target, data["frames"])

        if "pyramid" in data:
            self._write_pyramid(target, spatial_vol, data)

        self._write_mips(target, volume)

        if "device_data" in data:
            self._write_device_data(target, data["device_data"])

        if "provenance" in data:
            self._write_provenance(target, data["provenance"])

    # ------------------------------------------------------------------
    # Volume
    # ------------------------------------------------------------------

    def _write_volume(
        self,
        target: h5py.File | h5py.Group,
        volume: np.ndarray,
        data: dict[str, Any],
    ) -> None:
        ndim = volume.ndim
        chunks = (1,) * (ndim - 2) + volume.shape[-2:]
        ds = target.create_dataset(
            "volume",
            data=volume,
            chunks=chunks,
            compression="gzip",
            compression_opts=_GZIP_LEVEL,
        )
        ds.attrs["affine"] = data["affine"]
        ds.attrs["dimension_order"] = data["dimension_order"]
        ds.attrs["reference_frame"] = data["reference_frame"]
        ds.attrs["description"] = data["description"]

    # ------------------------------------------------------------------
    # Frames (4D+ data)
    # ------------------------------------------------------------------

    def _write_frames(
        self,
        target: h5py.File | h5py.Group,
        frames: dict[str, Any],
    ) -> None:
        grp = target.create_group("frames")
        grp.attrs["n_frames"] = np.int64(frames["n_frames"])
        grp.attrs["frame_type"] = frames["frame_type"]
        grp.attrs["description"] = frames["description"]

        grp.create_dataset(
            "frame_start",
            data=np.asarray(frames["frame_start"], dtype=np.float64),
        )
        grp["frame_start"].attrs["units"] = "s"
        grp["frame_start"].attrs["unitSI"] = np.float64(1.0)
        grp["frame_start"].attrs["description"] = (
            "Start time of each frame relative to reference"
        )

        grp.create_dataset(
            "frame_duration",
            data=np.asarray(frames["frame_duration"], dtype=np.float64),
        )
        grp["frame_duration"].attrs["units"] = "s"
        grp["frame_duration"].attrs["unitSI"] = np.float64(1.0)
        grp["frame_duration"].attrs["description"] = (
            "Duration of each frame (non-uniform allowed)"
        )

        if "frame_label" in frames:
            labels = frames["frame_label"]
            dt = h5py.special_dtype(vlen=str)
            grp.create_dataset("frame_label", data=labels, dtype=dt)
            grp["frame_label"].attrs["description"] = "Human-readable label per frame"

        if "gate_phase" in frames:
            ds = grp.create_dataset(
                "gate_phase",
                data=np.asarray(frames["gate_phase"], dtype=np.float64),
            )
            ds.attrs["units"] = "%"
            ds.attrs["description"] = "Phase within physiological cycle per gate bin"

        if "gate_trigger" in frames:
            self._write_gate_trigger(grp, frames["gate_trigger"])

    @staticmethod
    def _write_gate_trigger(
        frames_grp: h5py.Group,
        trigger: dict[str, Any],
    ) -> None:
        gt_grp = frames_grp.create_group("gate_trigger")

        ds = gt_grp.create_dataset(
            "signal",
            data=np.asarray(trigger["signal"], dtype=np.float64),
        )
        ds.attrs["description"] = "Raw physiological gating signal"

        sr_grp = gt_grp.create_group("sampling_rate")
        sr_grp.attrs["value"] = np.float64(trigger["sampling_rate"])
        sr_grp.attrs["units"] = "Hz"
        sr_grp.attrs["unitSI"] = np.float64(1.0)

        ds_tt = gt_grp.create_dataset(
            "trigger_times",
            data=np.asarray(trigger["trigger_times"], dtype=np.float64),
        )
        ds_tt.attrs["units"] = "s"
        ds_tt.attrs["unitSI"] = np.float64(1.0)
        ds_tt.attrs["description"] = "Detected trigger timestamps"

    # ------------------------------------------------------------------
    # Pyramid
    # ------------------------------------------------------------------

    def _write_pyramid(
        self,
        target: h5py.File | h5py.Group,
        spatial_vol: np.ndarray,
        data: dict[str, Any],
    ) -> None:
        pyramid_cfg = data["pyramid"]
        scale_factors = pyramid_cfg["scale_factors"]
        method = pyramid_cfg["method"]

        grp = target.create_group("pyramid")
        grp.attrs["n_levels"] = np.int64(len(scale_factors))
        grp.attrs["scale_factors"] = np.array(scale_factors, dtype=np.int64)
        grp.attrs["method"] = method
        grp.attrs["description"] = (
            "Multiscale pyramid for progressive-resolution access"
        )

        affine = data["affine"]

        for i, factor in enumerate(scale_factors, start=1):
            level_vol = _downsample(spatial_vol, factor)
            level_affine = _scale_affine(affine, factor)

            level_grp = grp.create_group(f"level_{i}")
            chunks = (1,) + level_vol.shape[1:]
            ds = level_grp.create_dataset(
                "volume",
                data=level_vol,
                chunks=chunks,
                compression="gzip",
                compression_opts=_GZIP_LEVEL,
            )
            ds.attrs["affine"] = level_affine
            ds.attrs["scale_factor"] = np.int64(factor)
            ds.attrs["description"] = f"{factor}x downsampled volume"

    # ------------------------------------------------------------------
    # MIP projections (nested under /mips/ group)
    # ------------------------------------------------------------------

    def _write_mips(
        self,
        target: h5py.File | h5py.Group,
        volume: np.ndarray,
    ) -> None:
        """Write MIP projections under ``/mips/`` group.

        For 3D volumes, MIPs are 2D arrays. For 4D+ volumes, MIPs are
        N-D arrays preserving all leading (non-spatial) dimensions.
        Spatial axes are assumed to be the last three dimensions (Z, Y, X).
        """
        ndim = volume.ndim
        grp = target.create_group("mips")

        # Spatial axis indices (last 3 dims)
        ax_z = ndim - 3  # axial collapses Z
        ax_y = ndim - 2  # coronal collapses Y
        ax_x = ndim - 1  # sagittal collapses X

        mip_cor = volume.max(axis=ax_y).astype(np.float32)
        ds_cor = grp.create_dataset("coronal", data=mip_cor)
        ds_cor.attrs["projection_type"] = "mip"
        ds_cor.attrs["axis"] = np.int64(ax_y)
        ds_cor.attrs["description"] = "Coronal MIP (max along Y)"

        mip_sag = volume.max(axis=ax_x).astype(np.float32)
        ds_sag = grp.create_dataset("sagittal", data=mip_sag)
        ds_sag.attrs["projection_type"] = "mip"
        ds_sag.attrs["axis"] = np.int64(ax_x)
        ds_sag.attrs["description"] = "Sagittal MIP (max along X)"

        mip_ax = volume.max(axis=ax_z).astype(np.float32)
        ds_ax = grp.create_dataset("axial", data=mip_ax)
        ds_ax.attrs["projection_type"] = "mip"
        ds_ax.attrs["axis"] = np.int64(ax_z)
        ds_ax.attrs["description"] = "Axial MIP (max along Z)"

    # ------------------------------------------------------------------
    # Embedded device_data (optional, NXlog pattern)
    # ------------------------------------------------------------------

    @staticmethod
    def _write_device_data(
        target: h5py.File | h5py.Group,
        channels: dict[str, dict[str, Any]],
    ) -> None:
        dd_grp = target.create_group("device_data")
        dd_grp.attrs["description"] = "Device signals recorded during this acquisition"

        for name, ch in channels.items():
            ch_grp = dd_grp.create_group(name)
            ch_grp.attrs["_type"] = ch.get("_type", name)
            ch_grp.attrs["_version"] = np.int64(ch.get("_version", 1))
            ch_grp.attrs["description"] = ch["description"]

            if "model" in ch:
                ch_grp.attrs["model"] = ch["model"]
            if "measurement" in ch:
                ch_grp.attrs["measurement"] = ch["measurement"]
            if "run_control" in ch:
                ch_grp.attrs["run_control"] = np.bool_(ch["run_control"])

            sr_grp = ch_grp.create_group("sampling_rate")
            sr_grp.attrs["value"] = np.float64(ch["sampling_rate"])
            sr_grp.attrs["units"] = "Hz"
            sr_grp.attrs["unitSI"] = np.float64(1.0)

            sig_ds = ch_grp.create_dataset(
                "signal",
                data=np.asarray(ch["signal"], dtype=np.float64),
                compression="gzip",
                compression_opts=_GZIP_LEVEL,
            )
            sig_ds.attrs["units"] = ch["units"]
            sig_ds.attrs["unitSI"] = np.float64(ch["unitSI"])

            time_ds = ch_grp.create_dataset(
                "time",
                data=np.asarray(ch["time"], dtype=np.float64),
                compression="gzip",
                compression_opts=_GZIP_LEVEL,
            )
            time_ds.attrs["units"] = "s"
            time_ds.attrs["unitSI"] = np.float64(1.0)
            if "time_start" in ch:
                time_ds.attrs["start"] = ch["time_start"]

    # ------------------------------------------------------------------
    # Provenance (optional)
    # ------------------------------------------------------------------

    @staticmethod
    def _write_provenance(
        target: h5py.File | h5py.Group,
        provenance: dict[str, Any],
    ) -> None:
        prov_grp = target.require_group("provenance")

        if "dicom_header" in provenance:
            header_json = provenance["dicom_header"]
            if not isinstance(header_json, str):
                header_json = json.dumps(header_json)
            dt = h5py.special_dtype(vlen=str)
            ds = prov_grp.create_dataset("dicom_header", data=header_json, dtype=dt)
            ds.attrs["description"] = (
                "Full DICOM header from representative source file, round-trippable"
            )

        if "per_slice_metadata" in provenance:
            arr = provenance["per_slice_metadata"]
            ds = prov_grp.create_dataset("per_slice_metadata", data=arr)
            ds.attrs["description"] = (
                "Per-slice DICOM metadata (instance_number, slice_location, "
                "acquisition_time, image_position_patient)"
            )


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def _spatial_volume(volume: np.ndarray, dimension_order: str) -> np.ndarray:
    """Extract or derive the 3D spatial volume for MIP / pyramid computation.

    For 4D+ data, sum over all leading (non-spatial) dimensions.
    """
    n_spatial = 3  # Z, Y, X
    n_leading = volume.ndim - n_spatial
    result = volume
    for _ in range(n_leading):
        result = result.sum(axis=0)
    return result.astype(np.float32)


def _downsample(vol: np.ndarray, factor: int) -> np.ndarray:
    """Downsample a 3D volume by *factor* using stride-based subsampling."""
    return vol[::factor, ::factor, ::factor].copy()


def _scale_affine(affine: np.ndarray, factor: int) -> np.ndarray:
    """Scale the spatial part of an affine matrix by *factor*."""
    scaled = affine.copy()
    scaled[:3, :3] *= factor
    return scaled
