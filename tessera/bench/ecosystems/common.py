"""Shared scaffold for the cross-ecosystem I/O comparison (#143).

The driver (`run.py`) generates ONE synthetic volume + ONE synthetic table, then drives every
ecosystem adapter through the same operations and times them identically. Each adapter lives in
`adapters/<name>.py` and implements this contract:

    NAME: str                      # display name, e.g. "HDF5 (h5py)"
    CODEC: str                     # the codec/settings used, e.g. "gzip-4, 64^3 chunks"
    CAPS: dict                     # {"volume": bool, "table": bool, "swmr": bool}

    # Volume — vol is an int16 C-contiguous ndarray, shape (D, H, W):
    def write_volume(path: str, vol) -> None
    def read_volume(path: str)        -> "ndarray"   # full, == vol
    def read_volume_zslice(path, z)   -> "ndarray"   # one axial plane (H, W)

    # Table — cols is a dict[str, 1-D ndarray] of equal length:
    def write_table(path: str, cols: dict) -> None
    def read_table(path: str)         -> dict        # full
    def read_table_column(path, name) -> "ndarray"   # one column

`path` is a base path the adapter may suffix (e.g. ".h5", ".zarr", ".nii.gz"); the adapter returns
the real path it wrote from `write_*` is NOT required — the driver passes the same base path to the
matching read and the adapter re-derives the suffix. Unsupported modality → raise NotImplementedError
(the driver skips it per CAPS). Reads are warm (page-cache) → this measures decode+parse throughput,
not cold disk. Correctness is asserted (read back == written) before timing counts.
"""

from __future__ import annotations

import os
import time

import numpy as np

# Dataset sizes — chosen to match the Rust .tsra-vs-bare bench (bench_compare.rs) for continuity.
VOL_N = 256  # 256^3 int16 = 32 MiB raw
TABLE_ROWS = 1_000_000  # u8 + 2x f4 = ~15 MiB raw


def make_volume() -> np.ndarray:
    """A smooth CT-like int16 volume (gradient in z+y) — realistic compression, not a pure ramp."""
    n = VOL_N
    z = np.arange(n, dtype=np.int64)[:, None, None]
    y = np.arange(n, dtype=np.int64)[None, :, None]
    x = np.zeros((1, 1, n), dtype=np.int64)
    vol = (z * 8 + y * 2 - 1024 + x).astype("<i2")
    return np.ascontiguousarray(vol)


def make_table() -> dict:
    """A listmode-like table: u8 monotonic timestamp + two f4 energy columns."""
    r = TABLE_ROWS
    return {
        "t": np.arange(r, dtype="<u8"),
        "e0": (511.0 + (np.arange(r) % 7)).astype("<f4"),
        "e1": (510.0 - (np.arange(r) % 5)).astype("<f4"),
    }


def best(fn, iters: int) -> float:
    """Min wall-clock (seconds) over `iters` runs — the least-contended run on a busy box."""
    b = float("inf")
    for _ in range(iters):
        t = time.perf_counter()
        fn()
        b = min(b, time.perf_counter() - t)
    return b


def dir_or_file_bytes(path: str) -> int:
    """On-disk size of a file or (for Zarr-style stores) a directory tree."""
    if os.path.isdir(path):
        total = 0
        for root, _dirs, files in os.walk(path):
            for f in files:
                total += os.path.getsize(os.path.join(root, f))
        return total
    return os.path.getsize(path)
