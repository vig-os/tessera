"""CERN ROOT TTree adapter via uproot (pure-Python, no ROOT install).

ROOT is the HEP standard for event/listmode data — a TTree is columnar (one branch per field), so
it maps to the table modality (not volumes). uproot writes + reads TTrees with per-branch access,
the direct analogue of Parquet column projection / Vortex column take.
"""

from __future__ import annotations

import numpy as np
import uproot

NAME = "ROOT (uproot/TTree)"
CODEC = "zstd-3, TTree"
CAPS = {"volume": False, "table": True, "swmr": False}

_TREE = "events"


def path_for(base: str, modality: str) -> str:
    return base + ".root"


def write_table(base: str, cols: dict) -> None:
    with uproot.recreate(path_for(base, "table"), compression=uproot.ZSTD(3)) as f:
        f[_TREE] = {k: np.ascontiguousarray(v) for k, v in cols.items()}


def read_table(base: str) -> dict:
    t = uproot.open(path_for(base, "table"))[_TREE]
    return {name: np.asarray(t[name].array(library="np")) for name in t.keys()}


def read_table_column(base: str, name: str) -> np.ndarray:
    # real per-branch read — ROOT decodes only the requested branch's baskets
    return np.asarray(uproot.open(path_for(base, "table"))[_TREE][name].array(library="np"))


def write_volume(base, vol):
    raise NotImplementedError("ROOT TTree is tabular/event data")


def read_volume(base):
    raise NotImplementedError


def read_volume_zslice(base, z):
    raise NotImplementedError
