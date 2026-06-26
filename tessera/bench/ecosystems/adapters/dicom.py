"""DICOM (pydicom) adapter for the cross-ecosystem I/O comparison (#143).

Volume only — DICOM has no first-class concept of an int16+f4+f4 listmode table, so
the table funcs raise NotImplementedError and the driver skips them per CAPS.

The volume is written as ONE multi-frame DICOM SOP Instance (Multi-frame Grayscale
Word Secondary Capture Image Storage, 1.2.840.10008.5.1.4.1.1.7.2) with explicit-VR
little-endian transfer syntax and uncompressed signed-int16 pixel data laid out frame-
major. That keeps the bytes a flat `D * H * W * 2`-byte slab on disk — bit-identical
to `vol.tobytes()` — so `pixel_array.reshape(D, H, W)` round-trips exactly. No
modality-LUT / rescale-slope / window-level is set, so pydicom returns raw int16.

Compression: none. DICOM is overwhelmingly stored uncompressed in the wild, so
ratio ~= 1.0 here is a real, honest data point for the cross-ecosystem table — the
overhead vs raw is just the (tiny) header + per-element VR tags.

Slice read: `pydicom.dcmread(...).pixel_array[z]` builds the full int16 ndarray and
then takes plane `z`. Because the transfer syntax is uncompressed and frames are
contiguous on disk, decode is just a `np.frombuffer` over PixelData — no per-frame
codec invocation — but it still touches all D*H*W*2 bytes. A truly partial decode
would require the Basic Offset Table (encapsulated transfer syntaxes only) and is
not meaningful for uncompressed data, so this is the honest implementation.
"""

from __future__ import annotations

import numpy as np
import pydicom
from pydicom.dataset import FileDataset, FileMetaDataset
from pydicom.uid import ExplicitVRLittleEndian, generate_uid

NAME = "DICOM (pydicom)"
CODEC = "uncompressed multiframe, explicit-VR LE"
CAPS = {"volume": True, "table": False, "swmr": False}

# Multi-frame Grayscale Word Secondary Capture Image Storage — the canonical SOP for
# a 16-bit grayscale multi-frame "image" with no acquisition-physics constraints.
_SOP_CLASS_UID = "1.2.840.10008.5.1.4.1.1.7.2"


def path_for(base: str, modality: str) -> str:
    return base + ".dcm"


# ---- volume ----
def write_volume(base: str, vol: np.ndarray) -> None:
    if vol.dtype != np.dtype("<i2"):
        raise ValueError(f"DICOM adapter expects little-endian int16 volume, got {vol.dtype}")
    if vol.ndim != 3:
        raise ValueError(f"DICOM adapter expects a 3-D (D, H, W) volume, got shape {vol.shape}")
    d, h, w = vol.shape
    path = path_for(base, "volume")

    file_meta = FileMetaDataset()
    file_meta.MediaStorageSOPClassUID = _SOP_CLASS_UID
    file_meta.MediaStorageSOPInstanceUID = generate_uid()
    file_meta.TransferSyntaxUID = ExplicitVRLittleEndian
    file_meta.ImplementationClassUID = generate_uid()
    file_meta.ImplementationVersionName = "TESSERA_BENCH"

    ds = FileDataset(path, {}, file_meta=file_meta, preamble=b"\0" * 128)

    # Identity (type-1 across most IODs)
    ds.SOPClassUID = _SOP_CLASS_UID
    ds.SOPInstanceUID = file_meta.MediaStorageSOPInstanceUID
    ds.StudyInstanceUID = generate_uid()
    ds.SeriesInstanceUID = generate_uid()

    # Patient / Study / Series — synthetic but present so pydicom round-trips cleanly
    ds.PatientName = "Bench^Volume"
    ds.PatientID = "BENCH-0"
    ds.PatientBirthDate = ""
    ds.PatientSex = ""
    ds.StudyDate = "20240101"
    ds.StudyTime = "000000"
    ds.AccessionNumber = ""
    ds.ReferringPhysicianName = ""
    ds.StudyID = "1"
    ds.SeriesNumber = 1
    ds.InstanceNumber = 1
    ds.Modality = "OT"  # "Other" — the SC IOD accepts any modality

    # SC-image type-1 content fields
    ds.ContentDate = "20240101"
    ds.ContentTime = "000000"
    ds.ConversionType = "WSD"  # Workstation

    # Image Pixel module — the bit-exactness contract lives here.
    ds.SamplesPerPixel = 1
    ds.PhotometricInterpretation = "MONOCHROME2"
    ds.NumberOfFrames = d
    ds.Rows = h
    ds.Columns = w
    ds.BitsAllocated = 16
    ds.BitsStored = 16
    ds.HighBit = 15
    ds.PixelRepresentation = 1  # signed (two's-complement int16)

    # Flat little-endian byte dump, frames in z-major order — matches vol.tobytes()
    # exactly, so dcmread().pixel_array round-trips bit-for-bit.
    ds.PixelData = vol.tobytes()

    # pydicom 3.x: `enforce_file_format=True` writes a real Part-10 file (preamble
    # + DICM magic + file meta) instead of "like-original" raw-dataset mode.
    pydicom.dcmwrite(path, ds, enforce_file_format=True)


def read_volume(base: str) -> np.ndarray:
    ds = pydicom.dcmread(path_for(base, "volume"))
    arr = ds.pixel_array  # (frames, rows, cols) int16 for this config
    # Defensive: normalise to the contract dtype/shape even if pydicom returns a
    # native-endian view on a little-endian box (it does).
    return np.ascontiguousarray(arr, dtype="<i2")


def read_volume_zslice(base: str, z: int) -> np.ndarray:
    # See module docstring: pixel_array decodes all frames (cheap memcpy here because
    # the transfer syntax is uncompressed); slicing is just an ndarray view.
    ds = pydicom.dcmread(path_for(base, "volume"))
    plane = ds.pixel_array[z]
    return np.ascontiguousarray(plane, dtype="<i2")


# ---- table (unsupported — DICOM is volume-only here) ----
def write_table(base: str, cols: dict) -> None:
    raise NotImplementedError("DICOM adapter is volume-only in this bench")


def read_table(base: str) -> dict:
    raise NotImplementedError("DICOM adapter is volume-only in this bench")


def read_table_column(base: str, name: str) -> np.ndarray:
    raise NotImplementedError("DICOM adapter is volume-only in this bench")
