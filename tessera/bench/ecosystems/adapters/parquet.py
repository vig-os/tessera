"""Parquet (pyarrow) adapter for the cross-ecosystem I/O comparison (#143).

Parquet is table-only — there is no volume modality (volume funcs raise NotImplementedError
so the driver skips them per CAPS).

Table: one Parquet file with one column per dict entry, zstd-compressed. Dtypes are
preserved bit-exact: `t` stays uint64, `e0`/`e1` stay float32 (Arrow types uint64/float32
map 1:1 to Parquet physical types, no promotion). `read_table_column` uses Parquet's
column-projection (`pq.read_table(path, columns=[name])`) so only that column's pages
are read off disk — Parquet's headline strength.
"""

from __future__ import annotations

import numpy as np
import pyarrow as pa
import pyarrow.parquet as pq

NAME = "Parquet (pyarrow)"
CODEC = "zstd"
CAPS = {"volume": False, "table": True, "swmr": False}


def path_for(base: str, modality: str) -> str:
    return base + ".parquet"


# ---- volume (unsupported) ----
def write_volume(base: str, vol: np.ndarray) -> None:
    raise NotImplementedError("Parquet is table-only")


def read_volume(base: str) -> np.ndarray:
    raise NotImplementedError("Parquet is table-only")


def read_volume_zslice(base: str, z: int) -> np.ndarray:
    raise NotImplementedError("Parquet is table-only")


# ---- table ----
def write_table(base: str, cols: dict) -> None:
    # from_pydict preserves numpy dtypes 1:1 (uint64 -> uint64, float32 -> float32).
    table = pa.table({name: pa.array(arr) for name, arr in cols.items()})
    pq.write_table(table, path_for(base, "table"), compression="zstd")


def read_table(base: str) -> dict:
    table = pq.read_table(path_for(base, "table"))
    # zero_copy_only=False because Parquet decode produces a fresh buffer anyway;
    # the explicit numpy() call preserves the Arrow type's native numpy dtype (u8/f4).
    return {name: table.column(name).to_numpy(zero_copy_only=False) for name in table.column_names}


def read_table_column(base: str, name: str) -> np.ndarray:
    # Real Parquet column projection — only `name`'s column chunks are read off disk.
    table = pq.read_table(path_for(base, "table"), columns=[name])
    return table.column(name).to_numpy(zero_copy_only=False)
