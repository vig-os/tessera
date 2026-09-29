"""NeXus (h5py + NXclass conventions) adapter for the cross-ecosystem I/O comparison (#143).

NeXus is HDF5 plus a structural convention: every group carries an `NX_class` attr,
an `NXentry` group holds the measurement, and an `NXdata` group flags the signal
dataset via `@signal` (+ optional `@axes`). This adapter writes a real NeXus tree
so the bench measures HDF5 plus the NX convention overhead, not bare HDF5.

Volume: `/entry` (NXentry) / `data` (NXdata) with `@signal="data"`, chunked 64^3 gzip-4.
Table:  `/entry` (NXentry) / `events` (NXdata) with one chunked gzip-4 dataset per column.
Bit-exact: int16 / u8 / f4 preserved as native HDF5 types, no scaling.
"""

from __future__ import annotations

import h5py
import numpy as np

NAME = "NeXus (h5py/NXdata)"
# NeXus IS HDF5 + NXclass conventions, so it gets HDF5's variants -- including `shuffle`, which this
# adapter also omitted (#485). Reporting NeXus without the standard pairing would repeat the #487
# omission in a second place while claiming to have fixed it.
VARIANTS = {
    "default": "contiguous, uncompressed, NXdata",
    "tuned": {
        "volume": "64^3 chunks, shuffle+gzip-4, NXdata",
        "table": "65536-row chunks (1-D), shuffle+gzip-4, NXdata",
    },
    "checked": {
        "volume": "64^3 chunks, shuffle+gzip-4, fletcher32, NXdata",
        "table": "65536-row chunks (1-D), shuffle+gzip-4, fletcher32, NXdata",
    },
}
CAPS = {"volume": True, "table": True, "swmr": True}

INTEGRITY = {
    "variant": "checked",
    "mechanism": "fletcher32 (per-chunk checksum)",
    "detects": "corruption in the chunks you actually read",
    "does_not": "unkeyed, covers no metadata or NXclass structure; proves nothing about provenance",
}


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


_VOL_CHUNK = (64, 64, 64)
_TABLE_CHUNK = 1 << 16  # 64 Ki rows per column chunk


def path_for(base: str, modality: str) -> str:
    return base + ".nxs"


def _nxentry(f: h5py.File) -> h5py.Group:
    g = f.create_group("entry")
    g.attrs["NX_class"] = "NXentry"
    return g


def _nxdata(parent: h5py.Group, name: str, signal: str, axes=None) -> h5py.Group:
    g = parent.create_group(name)
    g.attrs["NX_class"] = "NXdata"
    g.attrs["signal"] = signal
    if axes is not None:
        g.attrs["axes"] = axes
    return g


# ---- volume ----
def write_volume(base: str, vol: np.ndarray, variant: str = "tuned") -> None:
    chunks = tuple(min(c, s) for c, s in zip(_VOL_CHUNK, vol.shape))
    with h5py.File(path_for(base, "volume"), "w") as f:
        f.attrs["NX_class"] = "NXroot"
        entry = _nxentry(f)
        data = _nxdata(entry, "data", signal="data", axes=["z", "y", "x"])
        data.create_dataset("data", data=vol, dtype=vol.dtype, **_opts(variant, chunks))


def read_volume(base: str, variant: str = "tuned") -> np.ndarray:
    with h5py.File(path_for(base, "volume"), "r") as f:
        return f["entry/data/data"][...]


def read_volume_zslice(base: str, z: int, variant: str = "tuned") -> np.ndarray:
    with h5py.File(path_for(base, "volume"), "r") as f:
        return f["entry/data/data"][z]


# ---- table ----
def write_table(base: str, cols: dict, variant: str = "tuned") -> None:
    # pick the first column as the NXdata `@signal` — purely conventional, all
    # columns are siblings inside the NXdata group and are read independently.
    signal_name = next(iter(cols))
    with h5py.File(path_for(base, "table"), "w") as f:
        f.attrs["NX_class"] = "NXroot"
        entry = _nxentry(f)
        events = _nxdata(entry, "events", signal=signal_name)
        for name, arr in cols.items():
            chunks = (min(_TABLE_CHUNK, arr.shape[0]),)
            events.create_dataset(
                name, data=arr, dtype=arr.dtype, **_opts(variant, chunks)
            )


def read_table(base: str, variant: str = "tuned") -> dict:
    with h5py.File(path_for(base, "table"), "r") as f:
        g = f["entry/events"]
        return {name: g[name][...] for name in g.keys()}


def read_table_column(base: str, name: str, variant: str = "tuned") -> np.ndarray:
    with h5py.File(path_for(base, "table"), "r") as f:
        return f["entry/events"][name][...]
