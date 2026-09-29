"""HDF5 (h5py) adapter for the cross-ecosystem I/O comparison (#143, variants added in #485).

Variants
--------
default   h5py's own default: contiguous, uncompressed, no filters. What you get from
          `create_dataset(data=...)` with nothing else specified.
tuned     The STANDARD HDF5 pairing: 64^3 cubic chunks (volume) / 64Ki-row chunks (table),
          **shuffle** + gzip-4.
checked   `tuned` plus the `fletcher32` per-chunk checksum filter — the integrity row.

Why `shuffle` is not optional (#485)
------------------------------------
This adapter previously wrote `shuffle=False` with gzip-4 on both modalities. #487 measured what
that omission costs: adding shuffle took HDF5's tuned table from 5.9 MiB to 155.3 KiB and
**inverted the headline** of that comparison. Shuffle+deflate is the pairing every HDF5 guide
recommends for numeric data, so omitting it is not a neutral default — it hands the comparison to
whatever format it is being compared against. It is the standard tuning and it is used.

Bit-exact: int16 / u8 / f4 are preserved as native HDF5 types with no scaling.
"""

from __future__ import annotations

import h5py
import numpy as np

NAME = "HDF5 (h5py)"
VARIANTS = {
    "default": "contiguous, uncompressed",
    "tuned": {
        "volume": "64^3 chunks, shuffle+gzip-4",
        "table": "65536-row chunks (1-D), shuffle+gzip-4",
    },
    "checked": {
        "volume": "64^3 chunks, shuffle+gzip-4, fletcher32",
        "table": "65536-row chunks (1-D), shuffle+gzip-4, fletcher32",
    },
}
CAPS = {"volume": True, "table": True, "swmr": True}

INTEGRITY = {
    "variant": "checked",
    "mechanism": "fletcher32 (per-chunk checksum)",
    "detects": "corruption in the chunks you actually read",
    "does_not": "unkeyed and covers no metadata — an attacker who rewrites a chunk rewrites its "
    "checksum; proves nothing about provenance or which artifact this is",
}

_VOL_CHUNK = (64, 64, 64)
_TABLE_CHUNK = 1 << 16  # 64 Ki rows per column chunk


def path_for(base: str, modality: str) -> str:
    return base + ".h5"


def _opts(variant: str, chunks) -> dict:
    if variant == "default":
        return {}
    kw = {
        "chunks": chunks,
        "compression": "gzip",
        "compression_opts": 4,
        "shuffle": True,
    }
    if variant == "checked":
        kw["fletcher32"] = True
    return kw


# ---- volume ----
def write_volume(base: str, vol: np.ndarray, variant: str = "tuned") -> None:
    chunks = tuple(min(c, s) for c, s in zip(_VOL_CHUNK, vol.shape))
    with h5py.File(path_for(base, "volume"), "w") as f:
        f.create_dataset("volume", data=vol, dtype=vol.dtype, **_opts(variant, chunks))


def read_volume(base: str, variant: str = "tuned") -> np.ndarray:
    with h5py.File(path_for(base, "volume"), "r") as f:
        return f["volume"][...]


def read_volume_zslice(base: str, z: int, variant: str = "tuned") -> np.ndarray:
    with h5py.File(path_for(base, "volume"), "r") as f:
        return f["volume"][z]


# ---- table ----
def write_table(base: str, cols: dict, variant: str = "tuned") -> None:
    with h5py.File(path_for(base, "table"), "w") as f:
        g = f.create_group("events")
        for name, arr in cols.items():
            chunks = (min(_TABLE_CHUNK, arr.shape[0]),)
            g.create_dataset(name, data=arr, dtype=arr.dtype, **_opts(variant, chunks))


def read_table(base: str, variant: str = "tuned") -> dict:
    with h5py.File(path_for(base, "table"), "r") as f:
        g = f["events"]
        return {name: g[name][...] for name in g.keys()}


def read_table_column(base: str, name: str, variant: str = "tuned") -> np.ndarray:
    with h5py.File(path_for(base, "table"), "r") as f:
        return f["events"][name][...]
