"""DICOM (pydicom) adapter for the cross-ecosystem I/O comparison (#143).

Volume only — DICOM has no first-class concept of an int16+f4+f4 listmode table, so
the table funcs raise NotImplementedError and the driver skips them per CAPS.

The volume is written as ONE multi-frame DICOM SOP Instance (Multi-frame Grayscale
Word Secondary Capture Image Storage, 1.2.840.10008.5.1.4.1.1.7.2) with explicit-VR
little-endian transfer syntax and uncompressed signed-int16 pixel data laid out frame-
major. That keeps the bytes a flat `D * H * W * 2`-byte slab on disk — bit-identical
to `vol.tobytes()` — so `pixel_array.reshape(D, H, W)` round-trips exactly. No
modality-LUT / rescale-slope / window-level is set, so pydicom returns raw int16.

Two variants (#485):

- `default` — uncompressed. DICOM is overwhelmingly stored uncompressed in the wild, so ratio ~= 1.0
  is a real, honest data point: the overhead vs raw is just the header + per-element VR tags.
- `tuned` — RLE Lossless (encapsulated), the standard lossless transfer syntax pydicom 3 encodes
  natively. Verified bit-exact before it was listed.

Slice read: `pydicom.dcmread(...).pixel_array[z]` builds the FULL int16 ndarray, then takes plane `z`,
in both variants. Uncompressed, that is a `np.frombuffer` over contiguous PixelData — no codec, but it
still touches all D*H*W*2 bytes. Under RLE every frame is decoded before the plane is selected, so the
"slice" row costs the same as a full read (visible in the results). A truly partial decode would need
per-frame fragment access through the Basic Offset Table; pydicom's `pixel_array` does not do that, so
this is the honest implementation of what a user of the library gets, and the row says so by its speed.
"""

from __future__ import annotations

import numpy as np
import pydicom
from pydicom.dataset import FileDataset, FileMetaDataset
from pydicom.uid import ExplicitVRLittleEndian, RLELossless, generate_uid

NAME = "DICOM (pydicom)"
# DICOM's standard lossless tuning is an encapsulated transfer syntax. RLE Lossless is the one
# pydicom 3 encodes natively (verified bit-exact before being listed here, per #485) -- no extra
# plugin, no JPEG-LS dependency. Left as the tuned variant rather than a promise.
VARIANTS = {
    "default": "uncompressed multiframe, explicit-VR LE",
    "tuned": "RLE Lossless (encapsulated), explicit-VR LE",
}
CAPS = {"volume": True, "table": False, "swmr": False}

# Multi-frame Grayscale Word Secondary Capture Image Storage — the canonical SOP for
# a 16-bit grayscale multi-frame "image" with no acquisition-physics constraints.
_SOP_CLASS_UID = "1.2.840.10008.5.1.4.1.1.7.2"


def path_for(base: str, modality: str) -> str:
    return base + ".dcm"


# ---- volume ----
def write_volume(base: str, vol: np.ndarray, variant: str = "tuned") -> None:
    if vol.dtype != np.dtype("<i2"):
        raise ValueError(
            f"DICOM adapter expects little-endian int16 volume, got {vol.dtype}"
        )
    if vol.ndim != 3:
        raise ValueError(
            f"DICOM adapter expects a 3-D (D, H, W) volume, got shape {vol.shape}"
        )
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

    if variant == "tuned":
        # RLE Lossless: DICOM's standard lossless encapsulated syntax, encoded natively by
        # pydicom 3 (no plugin). Verified bit-exact before it was listed as a variant -- the
        # tuning is offered because it works here, not because the format nominally allows it.
        ds.compress(RLELossless)

    # pydicom 3.x: `enforce_file_format=True` writes a real Part-10 file (preamble
    # + DICM magic + file meta) instead of "like-original" raw-dataset mode.
    pydicom.dcmwrite(path, ds, enforce_file_format=True)


def read_volume(base: str, variant: str = "tuned") -> np.ndarray:
    ds = pydicom.dcmread(path_for(base, "volume"))
    arr = ds.pixel_array  # (frames, rows, cols) int16 for this config
    # Defensive: normalise to the contract dtype/shape even if pydicom returns a
    # native-endian view on a little-endian box (it does).
    return np.ascontiguousarray(arr, dtype="<i2")


def read_volume_zslice(base: str, z: int, variant: str = "tuned") -> np.ndarray:
    # See module docstring: pixel_array decodes all frames (cheap memcpy here because
    # the transfer syntax is uncompressed); slicing is just an ndarray view.
    ds = pydicom.dcmread(path_for(base, "volume"))
    plane = ds.pixel_array[z]
    return np.ascontiguousarray(plane, dtype="<i2")


# ---- table (unsupported — DICOM is volume-only here) ----
def write_table(base: str, cols: dict, variant: str = "tuned") -> None:
    raise NotImplementedError("DICOM adapter is volume-only in this bench")


def read_table(base: str, variant: str = "tuned") -> dict:
    raise NotImplementedError("DICOM adapter is volume-only in this bench")


def read_table_column(base: str, name: str, variant: str = "tuned") -> np.ndarray:
    raise NotImplementedError("DICOM adapter is volume-only in this bench")
