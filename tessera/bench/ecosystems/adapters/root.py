"""CERN ROOT adapter via uproot (pure-Python, no ROOT install).

ROOT is the HEP standard for event/listmode data. Both of its columnar containers are measured
here, because they are different formats with different performance — one branch/field per column
either way, the direct analogue of Parquet column projection / Vortex column take.

Variants
--------
default   **TTree** at uproot's default compression — the classic, ubiquitous ROOT container.
tuned     **TTree** at zstd-3.
rntuple   **RNTuple** at zstd-3 — ROOT's modern columnar container.

A labelling bug this fixes (#485)
---------------------------------
This adapter shipped as `NAME = "ROOT (uproot/TTree)"` with `CODEC = "zstd-3, TTree"`, and its
docstring said "uproot writes + reads TTrees". It did not. In uproot 5.7, assigning a dict
(`f["events"] = {...}`) dispatches to `mkrntuple` and writes a **`ROOT::RNTuple`** — verified by
reading `classname` back. So every published ROOT row was RNTuple's numbers under TTree's name.

That is the same class as #503's false "default" label, with one difference worth recording: it ran
**against** Tessera rather than for it. On the contract fixture RNTuple was ~11% smaller than TTree
(243,233 vs 273,857 bytes), so the mislabel flattered ROOT. The fairness audit is not one-directional.

A real TTree needs `mktree` + `extend`, which is what `default`/`tuned` now use.
`test_adapters.py::test_name_does_not_claim_a_container_the_writer_does_not_produce` reads the
container back and fails if a variant's settings string names one the file does not contain, so this
cannot silently regress.

Note: `uproot.recreate(..., compression=None)` raises `AttributeError: 'NoneType' object has no
attribute 'code'` inside the RNTuple writer — uproot has no uncompressed path here, so no
"uncompressed" variant is offered rather than one being faked.
"""

from __future__ import annotations

import numpy as np
import uproot

NAME = "ROOT (uproot)"
VARIANTS = {
    "default": "TTree, uproot default compression",
    "tuned": "TTree, zstd-3",
    "rntuple": "RNTuple, zstd-3",
}
CAPS = {"volume": False, "table": True, "swmr": False}

_TREE = "events"


def path_for(base: str, modality: str) -> str:
    return base + ".root"


def _compression(variant: str):
    return None if variant == "default" else uproot.ZSTD(3)


def write_table(base: str, cols: dict, variant: str = "tuned") -> None:
    data = {k: np.ascontiguousarray(v) for k, v in cols.items()}
    kw = {}
    comp = _compression(variant)
    if comp is not None:
        kw["compression"] = comp
    with uproot.recreate(path_for(base, "table"), **kw) as f:
        if variant == "rntuple":
            f[_TREE] = data  # dict assignment dispatches to mkrntuple -> ROOT::RNTuple
        else:
            f.mktree(_TREE, {k: v.dtype for k, v in data.items()})
            f[_TREE].extend(data)


def read_table(base: str, variant: str = "tuned") -> dict:
    with uproot.open(path_for(base, "table")) as f:
        t = f[_TREE]
        return {name: np.asarray(t[name].array(library="np")) for name in t.keys()}


def read_table_column(base: str, name: str, variant: str = "tuned") -> np.ndarray:
    # Real per-branch/per-field read — ROOT decodes only the requested column's pages/baskets.
    with uproot.open(path_for(base, "table")) as f:
        return np.asarray(f[_TREE][name].array(library="np"))


def write_volume(base, vol, variant: str = "tuned"):
    raise NotImplementedError("ROOT TTree/RNTuple is tabular/event data")


def read_volume(base, variant: str = "tuned"):
    raise NotImplementedError


def read_volume_zslice(base, z, variant: str = "tuned"):
    raise NotImplementedError
