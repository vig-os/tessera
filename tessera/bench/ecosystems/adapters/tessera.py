"""Tessera `.tsra` adapter — the subject of the comparison, not one of the ecosystems.

Uses the real `tessera` Python extension (tessera.so, built from tessera-py) — the same read/write
path a user gets. Volume → pcodec array block; table → Vortex table block; both sealed in a zip64
`.tsra` with a blake3 manifest. Volume slicing uses the real chunked partial-read
(`read_array_subset`); the table column read uses Vortex projection (`read_table_column`), which
touches only that column's segments.

One variant, and why (#485 / #487's "no invented knobs")
-------------------------------------------------------
Every other adapter here is reported at a default AND its standard tuning. Tessera is reported at
one setting because **the Python binding exposes no codec lever**: `tessera-py`'s `add_array`
hard-codes the array codec (`crates/tessera-py/src/lib.rs:260` — `spec.codec = "pcodec".into()`)
and its signature takes no codec argument, and the table backend has no user-facing codec knob at
all. #487 could report a tuned Tessera row because the **Rust** API exposes that choice; this
binding does not.

Fabricating a second row to satisfy the symmetry requirement would be the same dishonesty as
omitting a real one, so the asymmetry is declared instead. Note which way it cuts: it is a
**disadvantage to disclose**, not an excuse — a reader tuning HDF5 or Parquet for their data has
knobs that a Tessera user driving it from Python does not.
"""

from __future__ import annotations

import json

import numpy as np

import tessera  # tessera.so on sys.path (the bench dir)

NAME = "Tessera (.tsra)"
VARIANTS = {"default": "pcodec array / Vortex table, zip64+blake3"}
SINGLE_VARIANT_REASON = (
    "the Python binding exposes no codec knob: add_array hard-codes pcodec "
    "(tessera-py/src/lib.rs:260) and the table backend has no user-facing codec setting"
)
CAPS = {
    "volume": True,
    "table": True,
    "swmr": False,
}  # immutable-sealed; concurrent readers, no live append

INTEGRITY = {
    "variant": "default",
    "mechanism": "blake3 Merkle seal + `verify` (re-derives every block digest)",
    "detects": "any modification to any block, and to the metadata and provenance the manifest "
    "hash also covers",
    "does_not": "answer WHO produced it on its own — that needs the ADR-0037 signature binding the "
    "manifest hash to a signer; verify alone proves integrity, not authorship",
}


def path_for(base: str, modality: str, variant: str = "default") -> str:
    return base + ".tsra"


# ---- volume ----
def write_volume(base: str, vol: np.ndarray, variant: str = "default") -> None:
    b = tessera.Builder("recon", "bench", "ecosystem bench", "2024-01-01T00:00:00Z")
    b.add_array("volume", "i2", list(vol.shape), vol.tobytes())
    b.pack(path_for(base, "volume"))


def read_volume(base: str, variant: str = "default") -> np.ndarray:
    buf, shape, code = tessera.open(path_for(base, "volume")).read_array("volume")
    return np.frombuffer(buf, "<" + code).reshape(tuple(shape))


def read_volume_zslice(base: str, z: int, variant: str = "default") -> np.ndarray:
    r = tessera.open(path_for(base, "volume"))
    _d, h, w = _shape(r)
    buf, sshape, code = r.read_array_subset("volume", [z, 0, 0], [1, h, w])
    return np.frombuffer(buf, "<" + code).reshape(tuple(sshape))[0]


def _shape(r):
    m = json.loads(r.manifest_json())
    return tuple(b["spec"]["shape"] for b in m["blocks"] if b["name"] == "volume")[0]


# ---- table ----
_CODES = {"t": "u8", "e0": "f4", "e1": "f4"}


def write_table(base: str, cols: dict, variant: str = "default") -> None:
    b = tessera.Builder("listmode", "bench", "ecosystem bench", "2024-01-01T00:00:00Z")
    b.add_table("events", [(k, _CODES[k], v.tobytes()) for k, v in cols.items()], "t")
    b.pack(path_for(base, "table"))


def read_table(base: str, variant: str = "default") -> dict:
    cols = tessera.open(path_for(base, "table")).read_table("events")
    return {name: np.frombuffer(buf, "<" + code) for name, buf, code in cols}


def read_table_column(base: str, name: str, variant: str = "default") -> np.ndarray:
    # Vortex projection — reads only this column's segments.
    buf, code = tessera.open(path_for(base, "table")).read_table_column("events", name)
    return np.frombuffer(buf, "<" + code)


# ---- integrity ----
def verify(base: str, modality: str, variant: str = "default") -> None:
    """Full seal verification — re-derives every block digest against the sealed manifest."""
    tessera.verify(path_for(base, modality, variant))
