"""HDF5 (h5py) adapter for the cross-ecosystem I/O comparison (#143).

Volume: one chunked gzip-4 dataset, 64^3 chunks. Slice read uses h5py lazy
slicing (`ds[z]`) so only the chunks touching that plane decompress.

Table: each column stored as its OWN chunked gzip-4 dataset inside a single
group, so single-column reads touch exactly one dataset (fair columnar layout).

Bit-exact: int16 / u8 / f4 are preserved as native HDF5 types with no scaling.
"""

from __future__ import annotations

import h5py
import numpy as np

NAME = "HDF5 (h5py)"
CODEC = "gzip-4, 64^3 chunks"
CAPS = {"volume": True, "table": True, "swmr": True}

_VOL_CHUNK = (64, 64, 64)
_TABLE_CHUNK = 1 << 16  # 64 Ki rows per column chunk


def path_for(base: str, modality: str) -> str:
    return base + ".h5"


# ---- volume ----
def write_volume(base: str, vol: np.ndarray) -> None:
    chunks = tuple(min(c, s) for c, s in zip(_VOL_CHUNK, vol.shape))
    with h5py.File(path_for(base, "volume"), "w") as f:
        f.create_dataset(
            "volume",
            data=vol,
            dtype=vol.dtype,
            chunks=chunks,
            compression="gzip",
            compression_opts=4,
            shuffle=False,
        )


def read_volume(base: str) -> np.ndarray:
    with h5py.File(path_for(base, "volume"), "r") as f:
        return f["volume"][...]


def read_volume_zslice(base: str, z: int) -> np.ndarray:
    with h5py.File(path_for(base, "volume"), "r") as f:
        return f["volume"][z]


# ---- table ----
def write_table(base: str, cols: dict) -> None:
    with h5py.File(path_for(base, "table"), "w") as f:
        g = f.create_group("events")
        for name, arr in cols.items():
            chunks = (min(_TABLE_CHUNK, arr.shape[0]),)
            g.create_dataset(
                name,
                data=arr,
                dtype=arr.dtype,
                chunks=chunks,
                compression="gzip",
                compression_opts=4,
                shuffle=False,
            )


def read_table(base: str) -> dict:
    with h5py.File(path_for(base, "table"), "r") as f:
        g = f["events"]
        return {name: g[name][...] for name in g.keys()}


def read_table_column(base: str, name: str) -> np.ndarray:
    with h5py.File(path_for(base, "table"), "r") as f:
        return f["events"][name][...]
