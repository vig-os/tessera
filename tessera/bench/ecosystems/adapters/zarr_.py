"""Zarr v3 (zarr-python) adapter for the cross-ecosystem I/O comparison (#143, variants in #485).

Variants
--------
default   zstd-3, no byte filter — a plain, reasonable choice and what this adapter used to do
          unconditionally.
tuned     **blosc with the byte shuffle filter** (zstd inside blosc, shuffle on). This is Zarr's
          standard pairing for numeric data and the direct analogue of HDF5's shuffle+deflate; its
          omission is the same fairness gap #487 found for HDF5 and #503 for Parquet.

Volume → 64³ cubic chunks (matches Tessera's volume layout, so this compares codec stacks rather
than chunk geometry). `read_volume_zslice` uses `arr[z]`, which zarr resolves to the intersecting
chunks only. Table → a group with one array per column, so a column read touches exactly one
column's chunks.

The store is a v3 directory tree at `base + ".zarr"`; `common.dir_or_file_bytes` sizes the tree and
`common.evict` walks it (evicting one fd would leave the rest of the store warm while the row
claimed to be cold).
"""

from __future__ import annotations

import numpy as np
import zarr
from zarr.codecs import BloscCodec, BloscShuffle, ZstdCodec

NAME = "Zarr (zarr-python)"
VARIANTS = {
    "default": {
        "volume": "zstd-3, 64^3 chunks",
        "table": "zstd-3, 65536-row chunks (1-D)",
    },
    "tuned": {
        "volume": "blosc(zstd-3)+shuffle, 64^3 chunks",
        "table": "blosc(zstd-3)+shuffle, 65536-row chunks (1-D)",
    },
}
CAPS = {
    "volume": True,
    "table": True,
    "swmr": True,
}  # zarr stores are append-safe; concurrent readers fine

_ZSTD_LEVEL = 3
_VOL_CHUNK = (64, 64, 64)
_TABLE_CHUNK = 65_536


def path_for(base: str, modality: str) -> str:
    return base + ".zarr"


def _compressors(variant: str):
    if variant == "tuned":
        return [
            BloscCodec(cname="zstd", clevel=_ZSTD_LEVEL, shuffle=BloscShuffle.shuffle)
        ]
    return [ZstdCodec(level=_ZSTD_LEVEL)]


# ---- volume ----
def write_volume(base: str, vol: np.ndarray, variant: str = "tuned") -> None:
    # Clamp chunk shape to volume shape so a small VOL_N (e.g. the contract test's 32) still works.
    chunks = tuple(min(c, s) for c, s in zip(_VOL_CHUNK, vol.shape))
    arr = zarr.create_array(
        store=path_for(base, "volume"),
        shape=vol.shape,
        dtype=vol.dtype,
        chunks=chunks,
        compressors=_compressors(variant),
        overwrite=True,
    )
    arr[:] = vol


def read_volume(base: str, variant: str = "tuned") -> np.ndarray:
    return zarr.open(path_for(base, "volume"), mode="r")[:]


def read_volume_zslice(base: str, z: int, variant: str = "tuned") -> np.ndarray:
    # `arr[z]` is lazy: zarr fetches only the chunks intersecting the plane.
    return zarr.open(path_for(base, "volume"), mode="r")[z]


# ---- table ----
def write_table(base: str, cols: dict, variant: str = "tuned") -> None:
    g = zarr.open_group(path_for(base, "table"), mode="w")
    for name, data in cols.items():
        a = g.create_array(
            name,
            shape=data.shape,
            dtype=data.dtype,
            chunks=(min(_TABLE_CHUNK, data.shape[0]),),
            compressors=_compressors(variant),
            overwrite=True,
        )
        a[:] = data


def read_table(base: str, variant: str = "tuned") -> dict:
    g = zarr.open_group(path_for(base, "table"), mode="r")
    return {name: g[name][:] for name in g.array_keys()}


def read_table_column(base: str, name: str, variant: str = "tuned") -> np.ndarray:
    return zarr.open_group(path_for(base, "table"), mode="r")[name][:]
