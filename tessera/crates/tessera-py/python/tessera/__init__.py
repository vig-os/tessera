"""Tessera — FAIR data products, ergonomic Python surface.

The compiled extension (`tessera._native`, a pyo3/abi3 module) exposes the read/verify/write surface
returning raw ``(bytes, dtype_code[, shape])`` tuples. This pure-Python layer wraps it so an analyst
gets **native objects** — a NumPy ``ndarray`` for arrays, and a polars ``DataFrame`` / pyarrow
``Table`` / dict-of-arrays for tables — instead of decoding buffers by hand::

    import tessera
    r = tessera.open("study.tsra")
    vol = r.array("recon/volume")          # -> np.ndarray (C-order, native dtype)
    df  = r.table("events")                # -> polars.DataFrame
    ms  = r.column("events", "ms")         # -> np.ndarray (single column, projected)

NumPy is a hard dependency (arrays are ndarrays). polars and pyarrow are imported lazily, only when
``table()`` / ``table_arrow()`` are called — so the read+numpy path has no heavy deps.
"""

from __future__ import annotations

import json as _json

import numpy as _np

from . import _native
from ._native import TesseraError, __version__

__all__ = ["Reader", "Builder", "open", "verify", "TesseraError", "__version__"]


def _ndarray(buf: bytes, code: str, shape=None) -> "_np.ndarray":
    """Reconstruct a C-order, little-endian array from the FFI ``(bytes, numpy_code)``."""
    a = _np.frombuffer(buf, "<" + code)
    return a.reshape(tuple(shape)) if shape is not None else a


class Reader:
    """Ergonomic wrapper over the native reader — native objects instead of raw buffers."""

    def __init__(self, native):
        self._r = native

    # --- passthrough metadata / integrity ---
    @property
    def id(self) -> str:
        return self._r.id

    @property
    def product(self) -> str:
        return self._r.product

    def manifest(self) -> dict:
        """The manifest as a Python dict (parsed from the canonical JSON)."""
        return _json.loads(self._r.manifest_json())

    def manifest_json(self) -> str:
        return self._r.manifest_json()

    def block_names(self) -> list:
        return self._r.block_names()

    def verify(self) -> None:
        """Full integrity pass (seal + every block digest); raises ``TesseraError`` on mismatch."""
        return self._r.verify()

    def read_block(self, name: str) -> bytes:
        """The raw, digest-verified encoded bytes of a block (escape hatch)."""
        return self._r.read_block(name)

    def __repr__(self) -> str:
        return repr(self._r)

    # --- ergonomic reads ---
    def array(self, name: str) -> "_np.ndarray":
        """Decode an array block to a NumPy ``ndarray`` (C-order, native dtype)."""
        buf, shape, code = self._r.read_array(name)
        return _ndarray(buf, code, shape)

    def array_roi(self, name: str, origin, shape) -> "_np.ndarray":
        """Decode a 3-D ROI sub-cube — only the intersecting chunks are read + decoded."""
        buf, sshape, code = self._r.read_array_subset(name, list(origin), list(shape))
        return _ndarray(buf, code, sshape)

    def column(self, block: str, name: str) -> "_np.ndarray":
        """Decode one table column via Vortex projection (reads only that column's segments)."""
        buf, code = self._r.read_table_column(block, name)
        return _ndarray(buf, code)

    def logical_column(self, prefix: str, name: str) -> "_np.ndarray":
        """Decode one column of the *logical* multi-block table (`events`/`events_NNNN`), spanning
        every block in manifest order — the cross-block query, as a single ``ndarray``."""
        buf, code = self._r.read_logical_table_column(prefix, name)
        return _ndarray(buf, code)

    def table_dict(self, name: str) -> dict:
        """Decode a table block to a ``{column: np.ndarray}`` dict (no polars/pyarrow needed)."""
        return {n: _ndarray(buf, code) for n, buf, code in self._r.read_table(name)}

    def table_arrow(self, name: str):
        """Decode a table block to a pyarrow ``Table``."""
        import pyarrow as pa

        return pa.table(self.table_dict(name))

    def table(self, name: str):
        """Decode a table block to a polars ``DataFrame`` (via Arrow)."""
        import polars as pl

        return pl.from_arrow(self.table_arrow(name))


def open(path) -> Reader:  # noqa: A001 — shadowing builtins.open is the intended package API
    """Open a sealed ``.tsra`` (verifies magic + manifest seal) and return an ergonomic [`Reader`]."""
    return Reader(_native.open(str(path)))


def verify(path) -> None:
    """Full integrity pass over a ``.tsra`` (mirrors ``tessera verify``); raises on any mismatch."""
    return _native.verify(str(path))


# Write side: the native Builder (numpy buffers in, `pack()` out) is already ergonomic — re-export it.
Builder = _native.Builder
