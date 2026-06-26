"""Cross-ecosystem I/O comparison driver (#143).

Generates one synthetic volume + one table, drives every adapter through identical operations under
the SAME process (pin externally with `taskset`/`nice` for a clean slice), times min-of-N, asserts
correctness, and prints an ALOCA report + writes results.json.

Run (inside `nix develop`, from this dir):
    taskset -c 10-39 nice -n 19 uv run python run.py
"""

from __future__ import annotations

import argparse
import importlib
import json
import os
import sys
import tempfile
import traceback

import numpy as np

import common

# Adapter modules (in adapters/) and display order. Missing/broken ones are skipped with a note.
ADAPTERS = ["tessera", "hdf5", "zarr_", "nexus", "nifti", "dicom", "parquet", "root"]


def time_volume(mod, vol, tmp, iters):
    base = os.path.join(tmp, f"{mod.__name__.split('.')[-1]}_vol")
    mod.write_volume(base, vol)
    back = mod.read_volume(base)
    assert back.shape == vol.shape and back.dtype == vol.dtype, f"{mod.NAME}: vol shape/dtype"
    assert np.array_equal(back, vol), f"{mod.NAME}: vol not bit-exact"
    z = vol.shape[0] // 2
    sl = mod.read_volume_zslice(base, z)
    assert np.array_equal(sl, vol[z]), f"{mod.NAME}: zslice mismatch"
    size = common.dir_or_file_bytes(_resolve(mod, base, "volume"))
    return {
        "bytes": size,
        "ratio": vol.nbytes / size,
        "write_s": common.best(lambda: mod.write_volume(base, vol), iters),
        "read_full_s": common.best(lambda: mod.read_volume(base), iters),
        "read_slice_s": common.best(lambda: mod.read_volume_zslice(base, z), iters),
    }


def time_table(mod, cols, tmp, iters):
    base = os.path.join(tmp, f"{mod.__name__.split('.')[-1]}_tab")
    mod.write_table(base, cols)
    back = mod.read_table(base)
    for k, v in cols.items():
        assert k in back and np.array_equal(back[k], v), f"{mod.NAME}: table col {k} mismatch"
    one = mod.read_table_column(base, "e0")
    assert np.array_equal(one, cols["e0"]), f"{mod.NAME}: column read mismatch"
    raw = sum(v.nbytes for v in cols.values())
    size = common.dir_or_file_bytes(_resolve(mod, base, "table"))
    return {
        "bytes": size,
        "ratio": raw / size,
        "write_s": common.best(lambda: mod.write_table(base, cols), iters),
        "read_full_s": common.best(lambda: mod.read_table(base), iters),
        "read_col_s": common.best(lambda: mod.read_table_column(base, "e0"), iters),
    }


def _resolve(mod, base, modality):
    """Find the path the adapter actually wrote (it may suffix base)."""
    if hasattr(mod, "path_for"):
        return mod.path_for(base, modality)
    for cand in (base, base + ".h5", base + ".zarr", base + ".nii.gz", base + ".dcm",
                 base + ".parquet", base + ".tsra", base + ".nxs"):
        if os.path.exists(cand):
            return cand
    return base


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--vol-iters", type=int, default=3)
    ap.add_argument("--tab-iters", type=int, default=3)
    ap.add_argument("--only", default="", help="comma list to restrict adapters")
    ap.add_argument("--out", default="results.json")
    args = ap.parse_args()

    sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
    vol = common.make_volume()
    table = common.make_table()
    want = set(args.only.split(",")) if args.only else None

    results = {}
    for name in ADAPTERS:
        if want and name not in want:
            continue
        try:
            mod = importlib.import_module(f"adapters.{name}")
        except Exception as e:  # noqa: BLE001
            print(f"SKIP {name}: import failed — {e}", file=sys.stderr)
            continue
        entry = {"name": mod.NAME, "codec": getattr(mod, "CODEC", "?"),
                 "caps": mod.CAPS, "volume": None, "table": None}
        with tempfile.TemporaryDirectory() as tmp:
            if mod.CAPS.get("volume"):
                try:
                    entry["volume"] = time_volume(mod, vol, tmp, args.vol_iters)
                except Exception as e:  # noqa: BLE001
                    entry["volume"] = {"error": str(e)}
                    traceback.print_exc()
            if mod.CAPS.get("table"):
                try:
                    entry["table"] = time_table(mod, table, tmp, args.tab_iters)
                except Exception as e:  # noqa: BLE001
                    entry["table"] = {"error": str(e)}
                    traceback.print_exc()
        results[name] = entry

    with open(args.out, "w") as f:
        json.dump({"vol_mib": vol.nbytes / 2**20, "table_mib": sum(v.nbytes for v in table.values()) / 2**20,
                   "results": results}, f, indent=2)
    _print_aloca(vol, table, results)


def _mbps(nbytes, s):
    return (nbytes / 1e6) / s if s and s > 0 else float("nan")


def _print_aloca(vol, table, results):
    print(f"\n# Cross-ecosystem I/O — #143  (volume {vol.nbytes/2**20:.0f} MiB int16 {vol.shape}, "
          f"table {sum(v.nbytes for v in table.values())/2**20:.0f} MiB {len(table['t']):,} rows)")
    print("# warm (page-cache) reads → decode/parse throughput; min-of-N; one slice of the box.\n")

    print("## Volume")
    print(f"{'ecosystem':22} {'codec':22} {'ratio':>6} {'size':>9} {'write':>9} {'read':>9} {'slice':>9}")
    print(f"{'':22} {'':22} {'x':>6} {'MiB':>9} {'MB/s':>9} {'MB/s':>9} {'MB/s':>9}")
    for r in results.values():
        v = r.get("volume")
        if not v:
            continue
        if "error" in v:
            print(f"{r['name']:22} {r['codec']:22} {'ERR':>6}  {v['error'][:48]}")
            continue
        print(f"{r['name']:22} {r['codec']:22} {v['ratio']:>6.1f} {v['bytes']/2**20:>9.2f} "
              f"{_mbps(vol.nbytes, v['write_s']):>9.0f} {_mbps(vol.nbytes, v['read_full_s']):>9.0f} "
              f"{_mbps(vol.nbytes, v['read_slice_s']):>9.0f}")

    print("\n## Table")
    raw = sum(v.nbytes for v in table.values())
    print(f"{'ecosystem':22} {'codec':22} {'ratio':>6} {'size':>9} {'write':>9} {'read':>9} {'col':>9}")
    print(f"{'':22} {'':22} {'x':>6} {'MiB':>9} {'MB/s':>9} {'MB/s':>9} {'MB/s':>9}")
    for r in results.values():
        t = r.get("table")
        if not t:
            continue
        if "error" in t:
            print(f"{r['name']:22} {r['codec']:22} {'ERR':>6}  {t['error'][:48]}")
            continue
        print(f"{r['name']:22} {r['codec']:22} {t['ratio']:>6.1f} {t['bytes']/2**20:>9.2f} "
              f"{_mbps(raw, t['write_s']):>9.0f} {_mbps(raw, t['read_full_s']):>9.0f} "
              f"{_mbps(raw, t['read_col_s']):>9.0f}")

    swmr = [r["name"] for r in results.values() if r["caps"].get("swmr")]
    print(f"\nSWMR / concurrent-reader support: {', '.join(swmr) if swmr else 'none'}")


if __name__ == "__main__":
    main()
