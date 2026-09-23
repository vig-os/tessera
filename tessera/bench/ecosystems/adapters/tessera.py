"""Tessera `.tsra` adapter (reference implementation for the other ecosystem adapters).

Uses the real `tessera` Python extension (tessera.so, built from tessera-py) — the same read/write
path a user gets. Volume → pcodec array block; table → Vortex table block; both sealed in a zip64
`.tsra` with a blake3 manifest. Volume slicing uses the real chunked partial-read (read_array_subset);
the bindings don't expose single-column table projection yet, so a table "column read" is a full read.
"""

from __future__ import annotations

import numpy as np

import tessera  # tessera.so on sys.path (the bench dir)

NAME = "Tessera (.tsra)"
CODEC = "pcodec/Vortex, zip64+blake3"
CAPS = {"volume": True, "table": True, "swmr": False}  # immutable-sealed; concurrent readers, no live append


def path_for(base: str, modality: str) -> str:
    return base + ".tsra"


# ---- volume ----
def write_volume(base: str, vol: np.ndarray) -> None:
    b = tessera.Builder("recon", "bench", "ecosystem bench", "2024-01-01T00:00:00Z")
    b.add_array("volume", "i2", list(vol.shape), vol.tobytes())
    b.pack(path_for(base, "volume"))


def read_volume(base: str) -> np.ndarray:
    buf, shape, code = tessera.open(path_for(base, "volume")).read_array("volume")
    return np.frombuffer(buf, "<" + code).reshape(tuple(shape))


def read_volume_zslice(base: str, z: int) -> np.ndarray:
    r = tessera.open(path_for(base, "volume"))
    _, _, w = read_shape(r)
    buf, sshape, code = r.read_array_subset("volume", [z, 0, 0], [1, _h(r), w])
    return np.frombuffer(buf, "<" + code).reshape(tuple(sshape))[0]


def read_shape(r):
    import json
    m = json.loads(r.manifest_json())
    return tuple(b["spec"]["shape"] for b in m["blocks"] if b["name"] == "volume")[0]


def _h(r):
    return read_shape(r)[1]


# ---- table ----
_CODES = {"t": "u8", "e0": "f4", "e1": "f4"}


def write_table(base: str, cols: dict) -> None:
    b = tessera.Builder("listmode", "bench", "ecosystem bench", "2024-01-01T00:00:00Z")
    b.add_table("events", [(k, _CODES[k], v.tobytes()) for k, v in cols.items()], "t")
    b.pack(path_for(base, "table"))


def read_table(base: str) -> dict:
    cols = tessera.open(path_for(base, "table")).read_table("events")
    return {name: np.frombuffer(buf, "<" + code) for name, buf, code in cols}


def read_table_column(base: str, name: str) -> np.ndarray:
    # Vortex projection — reads only this column's segments (read_table_column binding)
    buf, code = tessera.open(path_for(base, "table")).read_table_column("events", name)
    return np.frombuffer(buf, "<" + code)
