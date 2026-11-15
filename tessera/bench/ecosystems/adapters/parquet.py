"""Parquet (pyarrow) adapter for the cross-ecosystem I/O comparison (#143, variants in #485).

Parquet is table-only — the volume funcs raise NotImplementedError and the driver skips them
per CAPS.

Variants
--------
default   snappy + dictionary + page statistics. This is the **pyarrow/Spark** default and what a
          reader gets without thinking about it.
tuned     zstd + **BYTE_STREAM_SPLIT** for the float columns, **dictionary off** for them. This is
          Parquet's counterpart to HDF5's shuffle; a dictionary over continuous floats defeats the
          split, so it is disabled where the split applies.
checked   `tuned` plus per-page CRC32 (`write_page_checksum`), verified on read — the integrity row.

Why `tuned` matters (#485): this adapter previously wrote plain `compression="zstd"` with no split
and the dictionary left on. #503 measured that omission on the Rust side: adding BYTE_STREAM_SPLIT
took tuned Parquet from 39.3 MiB to 23.4 MiB, turning a claimed 2.0x Tessera win into 1.19x.

Integrity, corrected (#485)
---------------------------
#503 recorded that Parquet page CRCs could not be timed because "parquet-rs 58's writer emits none".
That is a property of **parquet-rs**, not of Parquet. pyarrow 24 exposes `write_page_checksum` on
the writer and `page_checksum_verification` on the reader, and the round-trip is verified in this
harness's own tests. So the integrity row here measures a real mechanism, and the report attributes
the Rust-side gap to that implementation rather than to the format.

Dtypes are preserved bit-exact: `t` stays uint64, `e0`/`e1` stay float32 (Arrow uint64/float32 map
1:1 to Parquet physical types, no promotion).
"""

from __future__ import annotations

import numpy as np
import pyarrow as pa
import pyarrow.parquet as pq

NAME = "Parquet (pyarrow)"
VARIANTS = {
    # pyarrow 24's own default: snappy, dictionary on, and ONE row group for a 1M-row table
    # (verified: a default write of 1,000,000 rows produces a single row group). An earlier
    # revision forced 65,536-row groups here too, which made "default" not pyarrow's default.
    "default": "snappy + dictionary, 1 row group (pyarrow default)",
    "tuned": "zstd + BYTE_STREAM_SPLIT on floats, dict off, 65536-row groups",
    "checked": "zstd + BYTE_STREAM_SPLIT, 65536-row groups, page CRC32 verified",
}
CAPS = {"volume": False, "table": True, "swmr": False}

INTEGRITY = {
    "variant": "checked",
    "mechanism": "per-page CRC32 (pyarrow writes it; parquet-rs 58 does not)",
    "detects": "corruption within a data page that is read",
    "does_not": "unkeyed, covers no file-level metadata; proves nothing about provenance or "
    "which artifact this is",
}

_ROWS_PER_GROUP = 65_536


def path_for(base: str, modality: str) -> str:
    return base + ".parquet"


# ---- volume (unsupported) ----
def write_volume(base: str, vol: np.ndarray, variant: str = "tuned") -> None:
    raise NotImplementedError("Parquet is table-only")


def read_volume(base: str, variant: str = "tuned") -> np.ndarray:
    raise NotImplementedError("Parquet is table-only")


def read_volume_zslice(base: str, z: int, variant: str = "tuned") -> np.ndarray:
    raise NotImplementedError("Parquet is table-only")


# ---- table ----
def _write_opts(cols: dict, variant: str) -> dict:
    if variant == "default":
        # No row_group_size: pyarrow's default, which is what a user who does nothing gets.
        return {"compression": "snappy", "use_dictionary": True}
    floats = [n for n, a in cols.items() if a.dtype.kind == "f"]
    kw = {
        "compression": "zstd",
        # BYTE_STREAM_SPLIT is Parquet's shuffle analogue; a dictionary over continuous floats
        # defeats it, so the dictionary is disabled for exactly the columns that get the split.
        "column_encoding": {n: "BYTE_STREAM_SPLIT" for n in floats},
        "use_dictionary": [n for n in cols if n not in floats],
    }
    if variant == "checked":
        kw["write_page_checksum"] = True
    return kw


def write_table(base: str, cols: dict, variant: str = "tuned") -> None:
    # from_pydict preserves numpy dtypes 1:1 (uint64 -> uint64, float32 -> float32).
    table = pa.table({name: pa.array(arr) for name, arr in cols.items()})
    opts = _write_opts(cols, variant)
    if variant != "default":
        # Tuned variants use the same 65,536-row granularity HDF5/Zarr/NeXus chunk tables at, so
        # Parquet is compared on layout-equal terms rather than handed a free layout advantage.
        opts["row_group_size"] = _ROWS_PER_GROUP
    pq.write_table(table, path_for(base, "table"), **opts)


def _read_opts(variant: str) -> dict:
    return {"page_checksum_verification": True} if variant == "checked" else {}


def read_table(base: str, variant: str = "tuned") -> dict:
    table = pq.read_table(path_for(base, "table"), **_read_opts(variant))
    # zero_copy_only=False because Parquet decode produces a fresh buffer anyway; the explicit
    # numpy() call preserves the Arrow type's native numpy dtype (u8/f4).
    return {
        name: table.column(name).to_numpy(zero_copy_only=False)
        for name in table.column_names
    }


def read_table_column(base: str, name: str, variant: str = "tuned") -> np.ndarray:
    # Real Parquet column projection — only `name`'s column chunks are read off disk.
    table = pq.read_table(
        path_for(base, "table"), columns=[name], **_read_opts(variant)
    )
    return table.column(name).to_numpy(zero_copy_only=False)
