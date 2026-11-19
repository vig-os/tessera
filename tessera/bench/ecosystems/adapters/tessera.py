"""Tessera `.tsra` adapter — the subject of the comparison, not one of the ecosystems.

Uses the real `tessera` Python package (the pure-Python ergonomic layer over `tessera._native`,
built from tessera-py) — the same read/write path a user gets, via its PUBLIC API (`array`,
`array_roi`, `table_dict`, `column`), not the raw extension entry points underneath it. Volume → pcodec array block; table → Vortex table block; both sealed in a zip64
`.tsra` with a blake3 manifest. Volume slicing uses the real chunked partial-read
(`read_array_subset`); the table column read uses Vortex projection (`read_table_column`), which
touches only that column's segments.

Variants (#485, #533)
---------------------
default   pcodec for arrays — the binding's default and the settled imaging-volume codec.
tuned     `codec="auto"` for arrays: the writer encodes each array block with BOTH pcodec and zstd
          and keeps the smaller, recording the concrete winner in the manifest. That is the
          setting a user tunes to when they do not know their data's shape in advance, and it
          dominates a fixed `zstd` on size (it can only match or beat it).

The table backend (Vortex) has **no user-facing codec knob**, so both variants write tables
identically — the table settings string says so rather than implying a lever that does not exist.
Until #533 the Python binding hard-coded pcodec and this adapter declared a single-variant reason
saying so; that stopped being true when the binding gained its `codec` argument, so it is removed.
"""

from __future__ import annotations

import numpy as np

import tessera  # tessera.so on sys.path (the bench dir)

NAME = "Tessera (.tsra)"
VARIANTS = {
    "default": {
        "volume": "pcodec, 64^3 chunks, zip64+blake3",
        "table": "Vortex (no user codec knob), zip64+blake3",
    },
    "tuned": {
        "volume": "auto (pcodec|zstd, smaller kept), 64^3 chunks",
        "table": "Vortex — identical to default (no codec knob)",
    },
}
_ARRAY_CODEC = {"default": "pcodec", "tuned": "auto"}
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
    b.add_array("volume", "i2", list(vol.shape), vol.tobytes(), _ARRAY_CODEC[variant])
    b.pack(path_for(base, "volume"))


def read_volume(base: str, variant: str = "default") -> np.ndarray:
    return tessera.open(path_for(base, "volume")).array("volume")


def read_volume_zslice(base: str, z: int, variant: str = "default") -> np.ndarray:
    r = tessera.open(path_for(base, "volume"))
    _d, h, w = _shape(r)
    # Real chunked partial read — only the chunks intersecting plane z are fetched and decoded.
    return r.array_roi("volume", [z, 0, 0], [1, h, w])[0]


def _shape(r):
    blocks = r.manifest()["blocks"]
    return tuple(b["spec"]["shape"] for b in blocks if b["name"] == "volume")[0]


# ---- table ----
_CODES = {"t": "u8", "e0": "f4", "e1": "f4"}


def write_table(base: str, cols: dict, variant: str = "default") -> None:
    b = tessera.Builder("listmode", "bench", "ecosystem bench", "2024-01-01T00:00:00Z")
    b.add_table("events", [(k, _CODES[k], v.tobytes()) for k, v in cols.items()], "t")
    b.pack(path_for(base, "table"))


def read_table(base: str, variant: str = "default") -> dict:
    return tessera.open(path_for(base, "table")).table_dict("events")


def read_table_column(base: str, name: str, variant: str = "default") -> np.ndarray:
    # Vortex projection — reads only this column's segments.
    return tessera.open(path_for(base, "table")).column("events", name)


# ---- integrity ----
def verify(base: str, modality: str, variant: str = "default") -> None:
    """Full seal verification — re-derives every block digest against the sealed manifest."""
    tessera.verify(path_for(base, modality, variant))
