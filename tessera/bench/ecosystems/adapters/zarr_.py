"""Zarr v3 (zarr-python) adapter for the cross-ecosystem I/O comparison (#143).

Volume → one zarr array with 64³ cubic chunks + zstd compression (matches Tessera's volume layout,
so we're comparing codec-stack overhead, not chunk geometry). `read_volume_zslice` uses `arr[z]`,
which zarr resolves to the intersecting chunks only.

Table → a zarr group with one array per column, so `read_table_column` reads exactly one column's
chunks (the listmode-throughput question that motivates the bench). Each column gets the same
zstd level so the comparison is apples-to-apples with the volume codec choice.

The store is a v3 directory tree at `base + ".zarr"`; the driver sizes the whole tree via
`common.dir_or_file_bytes`.
"""

from __future__ import annotations

import numpy as np
import zarr
from zarr.codecs import ZstdCodec

NAME = "Zarr (zarr-python)"
CODEC = "zstd-3, 64^3 chunks"
CAPS = {"volume": True, "table": True, "swmr": True}  # zarr stores are append-safe; concurrent readers fine

_ZSTD_LEVEL = 3
_VOL_CHUNK = (64, 64, 64)
_TABLE_CHUNK = 65_536  # one chunk-per-65k-rows — small enough to be cache-friendly, big enough to compress well


def path_for(base: str, modality: str) -> str:
    return base + ".zarr"


# ---- volume ----
def write_volume(base: str, vol: np.ndarray) -> None:
    path = path_for(base, "volume")
    # Clamp chunk shape to volume shape so VOL_N < 64 (e.g. the self-test's 32) still works.
    chunks = tuple(min(c, s) for c, s in zip(_VOL_CHUNK, vol.shape))
    arr = zarr.create_array(
        store=path,
        shape=vol.shape,
        dtype=vol.dtype,
        chunks=chunks,
        compressors=[ZstdCodec(level=_ZSTD_LEVEL)],
        overwrite=True,
    )
    arr[:] = vol


def read_volume(base: str) -> np.ndarray:
    arr = zarr.open(path_for(base, "volume"), mode="r")
    return arr[:]


def read_volume_zslice(base: str, z: int) -> np.ndarray:
    arr = zarr.open(path_for(base, "volume"), mode="r")
    # `arr[z]` is lazy: zarr fetches only the chunks intersecting the plane.
    return arr[z]


# ---- table ----
def write_table(base: str, cols: dict) -> None:
    path = path_for(base, "table")
    g = zarr.open_group(path, mode="w")
    for name, data in cols.items():
        chunks = (min(_TABLE_CHUNK, data.shape[0]),)
        a = g.create_array(
            name,
            shape=data.shape,
            dtype=data.dtype,
            chunks=chunks,
            compressors=[ZstdCodec(level=_ZSTD_LEVEL)],
            overwrite=True,
        )
        a[:] = data


def read_table(base: str) -> dict:
    g = zarr.open_group(path_for(base, "table"), mode="r")
    return {name: g[name][:] for name in g.array_keys()}


def read_table_column(base: str, name: str) -> np.ndarray:
    g = zarr.open_group(path_for(base, "table"), mode="r")
    return g[name][:]
