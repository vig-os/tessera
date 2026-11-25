"""Cross-ecosystem I/O comparison driver (#143, method hardened in #485).

Generates one synthetic volume + BOTH table fixtures, drives every adapter through identical
operations at every VARIANT, times median-of-N with spread, repeats every read warm and cold, and
prints an ALOCA report + writes results.json.

Method, and why each piece is here
----------------------------------
median + spread   Replaces min-of-N. The minimum is a throughput ceiling; it hides variance and
                  cannot show when two formats are within noise of each other.
default AND tuned Every format at a sensible default and at its standard tuning, settings printed
                  on every row. #487 (HDF5 shuffle) and #503 (Parquet BYTE_STREAM_SPLIT) each found
                  that omitting a format's standard tuning flatters Tessera by a large factor.
both fixtures     periodic AND continuous, every row labelled (#497). Same encoder, same command,
                  opposite verdicts — publishing one is choosing the answer.
warm AND cold     Cold via fsync + POSIX_FADV_DONTNEED, with post-eviction residency MEASURED per
                  row. A cold-labelled row the kernel served from RAM is worse than no row.
correctness first Every read is verified against the source before its timing counts, so no format
                  can win by returning something cheaper.

Cold timing is not warm timing with an extra step: each cold read repopulates the page cache, so
the eviction is repeated before EVERY iteration and the worst residency across them is reported.

Run (inside `nix develop`, from this dir):
    taskset -c 10-39 nice -n 19 uv run python run.py          # N=15, as published
"""

from __future__ import annotations

import argparse
import importlib
import json
import os
import platform
import statistics
import sys
import time
import traceback

import numpy as np

import common

ADAPTERS = ["tessera", "hdf5", "zarr_", "nexus", "nifti", "dicom", "parquet", "root"]
REFERENCE = "tessera"


# --------------------------------------------------------------------------- helpers
def _resolve(mod, base, modality, variant):
    """The path the adapter actually wrote (it may suffix base, and the suffix may be variant-
    dependent — NIfTI's `.nii` vs `.nii.gz` is exactly that)."""
    if hasattr(mod, "path_for"):
        try:
            return mod.path_for(base, modality, variant)
        except TypeError:
            return mod.path_for(base, modality)
    for cand in (
        base,
        base + ".h5",
        base + ".zarr",
        base + ".nii",
        base + ".nii.gz",
        base + ".dcm",
        base + ".parquet",
        base + ".tsra",
        base + ".nxs",
        base + ".root",
    ):
        if os.path.exists(cand):
            return cand
    return base


def _summary(samples, worst=None):
    out = {
        "median": statistics.median(samples),
        "min": min(samples),
        "max": max(samples),
        "n": len(samples),
        "cold": worst is not None,
    }
    if worst is not None:
        # The MEASURED post-eviction residency, printed on every cold row: eviction is
        # best-effort and the kernel may decline, so the report says which it actually got (#487).
        out["worst_residency"] = worst
    return out


def _round_robin(fns, iters, paths=None):
    """Time several operations N times each, INTERLEAVED — sample i of every op before sample
    i+1 of any.

    Timing each variant as a contiguous block made results depend on what ran immediately
    before. Measured: Tessera's `auto` variant wrote a file BYTE-IDENTICAL to `default` (same
    sha256, same recorded codec) yet its reads timed 27% slower, because they ran straight after
    its own 15 double-encode writes — a heavy multi-threaded burst that depresses whatever is timed
    next. Interleaved, the two read in 62.7 vs 60.0 ms, overlapping. Round-robin spreads any drift
    evenly across the things being compared instead of charging it to whichever ran second.

    With `paths`, every sample is COLD: eviction is repeated before each one, because a cold read
    repopulates the cache and one eviction followed by N reads would time 1 cold + N-1 warm.
    """
    samples = {k: [] for k in fns}
    worst = {k: 0.0 for k in fns}
    for _ in range(iters):
        for k, fn in fns.items():
            if paths is not None:
                worst[k] = max(worst[k], common.evict(paths[k]))
            t0 = time.perf_counter()
            fn()
            samples[k].append(time.perf_counter() - t0)
    return {
        k: _summary(samples[k], worst[k] if paths is not None else None) for k in fns
    }


def _prepare(mod, variant, modality, fixture, data, root):
    """Write one config ONCE and verify it against the source before anything is timed.

    Correctness gates timing: a read that returns the wrong data never gets a number, so no
    format can win by returning something cheaper.
    """
    base = os.path.join(
        root, f"{mod.__name__.split('.')[-1]}_{variant}_{fixture}_{modality}"
    )
    if modality == "volume":
        mod.write_volume(base, data, variant)
        back = mod.read_volume(base, variant)
        assert back.shape == data.shape and back.dtype == data.dtype, (
            f"{mod.NAME}/{variant}: shape"
        )
        assert np.array_equal(back, data), (
            f"{mod.NAME}/{variant}/{fixture}: volume not bit-exact"
        )
        z = data.shape[0] // 2
        assert np.array_equal(mod.read_volume_zslice(base, z, variant), data[z]), (
            f"{mod.NAME}/{variant}/{fixture}: z-slice mismatch"
        )
        nbytes = data.nbytes
        ops = {
            "read_full": lambda: mod.read_volume(base, variant),
            "read_slice": lambda: mod.read_volume_zslice(base, z, variant),
        }
        write = lambda: mod.write_volume(base, data, variant)  # noqa: E731
    else:
        mod.write_table(base, data, variant)
        back = mod.read_table(base, variant)
        for k, v in data.items():
            assert k in back and np.array_equal(back[k], v), (
                f"{mod.NAME}/{variant}/{fixture}: {k}"
            )
        assert np.array_equal(mod.read_table_column(base, "e0", variant), data["e0"]), (
            f"{mod.NAME}/{variant}/{fixture}: projected column mismatch"
        )
        nbytes = sum(v.nbytes for v in data.values())
        ops = {
            "read_full": lambda: mod.read_table(base, variant),
            "read_col": lambda: mod.read_table_column(base, "e0", variant),
        }
        write = lambda: mod.write_table(base, data, variant)  # noqa: E731
    if hasattr(mod, "verify"):
        # A standalone integrity pass (tessera). HDF5 fletcher32 / Parquet CRCs are checked
        # DURING a read instead, so their cost shows as checked-vs-tuned read rows.
        mod.verify(
            base, modality, variant
        )  # must pass on the intact file before it is timed
        ops["verify"] = lambda: mod.verify(base, modality, variant)
    path = _resolve(mod, base, modality, variant)
    size = common.dir_or_file_bytes(path)
    return {
        "path": path,
        "nbytes": nbytes,
        "bytes": size,
        "ratio": nbytes / size,
        "ops": ops,
        "write": write,
    }


# --------------------------------------------------------------------------- environment
def environment() -> dict:
    """Recorded with every result set — absolute throughput is machine and slice dependent, so a
    number without its box is not reproducible and should not be quoted."""
    mods = {}
    for m in ("numpy", "h5py", "zarr", "pyarrow", "nibabel", "pydicom", "uproot"):
        try:
            mods[m] = importlib.import_module(m).__version__
        except Exception:  # noqa: BLE001
            mods[m] = "absent"
    try:
        with open("/proc/cpuinfo") as f:
            cpu = next(
                (
                    ln.split(":", 1)[1].strip()
                    for ln in f
                    if ln.startswith("model name")
                ),
                platform.processor(),
            )
    except OSError:
        cpu = platform.processor()
    return {
        "cpu": cpu,
        "cores": os.cpu_count(),
        "kernel": platform.release(),
        "python": platform.python_version(),
        "libraries": mods,
        "affinity": sorted(os.sched_getaffinity(0)),
    }


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument(
        "--iters",
        type=int,
        default=15,
        help="N for median-of-N (published results use 15)",
    )
    ap.add_argument("--only", default="", help="comma list to restrict adapters")
    ap.add_argument("--no-cold", action="store_true", help="skip cold-cache rows")
    ap.add_argument(
        "--markdown",
        default="",
        help="also write Markdown tables here; the best cell per column is bolded FROM THE DATA",
    )
    ap.add_argument("--out", default="results.json")
    ap.add_argument("--tmp", default="", help="scratch dir (defaults to system temp)")
    args = ap.parse_args()

    sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
    volumes = {k: common.make_volume(k) for k in common.VOLUMES}
    tables = {k: common.make_table(k) for k in common.FIXTURES}
    want = set(args.only.split(",")) if args.only else None
    do_cold = not args.no_cold

    # Three GLOBAL phases, so no timed read ever follows a timed-write burst (see _round_robin):
    #   0  prepare  — write every config once and verify it against the source
    #   1  reads    — all timed reads (warm, cold, verify), variants interleaved round-robin
    #   2  writes   — all timed writes, last, variants interleaved round-robin
    import shutil
    import tempfile

    root = tempfile.mkdtemp(prefix="tessera-bench-", dir=args.tmp or None)
    results, mods, prepared = {}, {}, {}
    try:
        # ---- phase 0: prepare
        for name in ADAPTERS:
            if want and name not in want:
                continue
            try:
                mod = importlib.import_module(f"adapters.{name}")
            except Exception as e:  # noqa: BLE001
                # The reference adapter missing invalidates the whole comparison; an ecosystem
                # adapter missing only drops its rows. Never let the first read as the second.
                if name == REFERENCE:
                    sys.exit(f"FATAL: reference adapter {name!r} not importable — {e}")
                print(f"SKIP {name}: import failed — {e}", file=sys.stderr)
                continue
            mods[name] = mod
            entry = {"name": mod.NAME, "caps": mod.CAPS, "variants": {}}
            if hasattr(mod, "INTEGRITY"):
                entry["integrity"] = mod.INTEGRITY
            if getattr(mod, "SINGLE_VARIANT_REASON", ""):
                entry["single_variant_reason"] = mod.SINGLE_VARIANT_REASON
            for variant in mod.VARIANTS:
                # Settings resolved PER MODALITY: volumes chunk 64^3, tables 1-D at 65536 rows,
                # and printing the cubic geometry on a table row would be a false claim (#487).
                v = {"volumes": {}, "tables": {}}
                if mod.CAPS.get("volume"):
                    v["settings_volume"] = common.settings_for(mod, variant, "volume")
                if mod.CAPS.get("table"):
                    v["settings_table"] = common.settings_for(mod, variant, "table")
                entry["variants"][variant] = v
                for modality, src, datasets in (
                    ("volume", "volumes", volumes),
                    ("table", "tables", tables),
                ):
                    if not mod.CAPS.get(modality):
                        continue
                    for fixture, data in datasets.items():
                        try:
                            cfg = _prepare(mod, variant, modality, fixture, data, root)
                            prepared[(name, variant, src, fixture)] = cfg
                            v[src][fixture] = {
                                "bytes": cfg["bytes"],
                                "ratio": cfg["ratio"],
                            }
                        except Exception as e:  # noqa: BLE001
                            v[src][fixture] = {"error": str(e)}
                            traceback.print_exc()
            results[name] = entry

        def groups():
            """(adapter, src, fixture) -> {variant: cfg}, only for configs that prepared OK."""
            g = {}
            for (name, variant, src, fixture), cfg in prepared.items():
                g.setdefault((name, src, fixture), {})[variant] = cfg
            return g

        # ---- phase 1: every timed read, variants interleaved
        for (name, src, fixture), cfgs in groups().items():
            ops = next(iter(cfgs.values()))["ops"]
            for op in ops:
                fns = {v: c["ops"][op] for v, c in cfgs.items()}
                for v, st in _round_robin(fns, args.iters).items():
                    results[name]["variants"][v][src][fixture][f"{op}_warm"] = st
                if do_cold:
                    paths = {v: c["path"] for v, c in cfgs.items()}
                    for v, st in _round_robin(fns, args.iters, paths).items():
                        results[name]["variants"][v][src][fixture][f"{op}_cold"] = st

        # ---- phase 2: every timed write, last
        for (name, src, fixture), cfgs in groups().items():
            fns = {v: c["write"] for v, c in cfgs.items()}
            for v, st in _round_robin(fns, args.iters).items():
                results[name]["variants"][v][src][fixture]["write"] = st
    finally:
        shutil.rmtree(root, ignore_errors=True)

    payload = {
        "environment": environment(),
        "iters": args.iters,
        "vol_mib": {k: v.nbytes / 2**20 for k, v in volumes.items()},
        "table_mib": {
            k: sum(c.nbytes for c in v.values()) / 2**20 for k, v in tables.items()
        },
        "results": results,
    }
    with open(args.out, "w") as f:
        json.dump(payload, f, indent=2)
    report(payload, volumes, tables)
    if args.markdown:
        with open(args.markdown, "w") as f:
            f.write(markdown(payload, volumes, tables))


# --------------------------------------------------------------------------- report
def _mbps(nbytes, s):
    return (nbytes / 1e6) / s if s and s > 0 else float("nan")


def _ms(st):
    """`median [lo-hi]` in milliseconds."""
    if not st:
        return "-"
    return f"{st['median'] * 1e3:.1f} [{st['min'] * 1e3:.1f}-{st['max'] * 1e3:.1f}]"


def _size_ratio(payload, a, b, src, fixture):
    """bytes(a) / bytes(b) for one fixture, from the results themselves; None if either is absent."""
    try:
        ra = payload["results"][a[0]]["variants"][a[1]][src][fixture]["bytes"]
        rb = payload["results"][b[0]]["variants"][b[1]][src][fixture]["bytes"]
    except (KeyError, TypeError):
        return None
    return ra / rb


def _cell(st, nbytes):
    """`median [lo-hi]` in MB/s — the spread is printed so a reader can see when two formats are
    within noise of each other. lo/hi come from the SLOWEST/FASTEST sample (max/min time)."""
    if not st:
        return "-"
    med = _mbps(nbytes, st["median"])
    lo, hi = _mbps(nbytes, st["max"]), _mbps(nbytes, st["min"])
    return f"{med:.0f} [{lo:.0f}-{hi:.0f}]"


def report(payload, volumes, tables):
    env = payload["environment"]
    _anyvol = next(iter(volumes.values()))
    print(
        f"\n# Cross-ecosystem I/O — #143/#485  (volumes {_anyvol.nbytes / 2**20:.0f} MiB int16 "
        f"{_anyvol.shape}; "
        f"tables {len(next(iter(tables.values()))['t']):,} rows)"
    )
    print(
        f"# {env['cpu']} x{env['cores']} · kernel {env['kernel']} · python {env['python']}"
    )
    print(f"# {' · '.join(f'{k} {v}' for k, v in env['libraries'].items())}")
    print(
        f"# median of N={payload['iters']}, [min..max] in results.json · MB/s over RAW bytes"
    )
    print(
        "# CAVEAT: synthetic data is far more compressible than real acquisitions — the size"
    )
    print(
        "#         column is a ratio BETWEEN formats on identical input, not an absolute claim.\n"
    )

    hdr = f"{'ecosystem':20} {'settings':52} {'ratio':>6} {'MiB':>8} {'write':>15} {'read':>15} {'slice':>15} {'cold-rd':>15} {'resid':>6}"
    for vtag, vol in volumes.items():
        print(f"\n## Volume — {vtag} fixture")
        if vtag == "gradient":
            print(
                "#  a pure linear ramp: CONSTANT deltas, so LZ77 matches run the length of the"
            )
            print(
                "#  array — ideal for deflate/zstd, adversarial for value-distribution codecs."
            )
        else:
            print(
                "#  anatomy + detector noise — what a real reconstruction contains. The noise"
            )
            print(
                "#  destroys long LZ matches but is exactly what a numeric codec models."
            )
        # Computed from THIS run, never hard-coded (a number in a header must come from the data
        # beside it). The size-dependence of the gradient's ratio is a separate measurement,
        # recorded where it was made: common.make_volume's docstring.
        cr = _size_ratio(
            payload, ("tessera", "default"), ("zarr_", "default"), "volumes", vtag
        )
        if cr is not None:
            print(
                f"#  this run: Tessera (pcodec) / Zarr (zstd-3) size = {cr:.2f}"
                f"  ({'pcodec smaller' if cr < 1 else 'zstd smaller'})"
            )
        print(hdr)
        for r in payload["results"].values():
            for v in r["variants"].values():
                m = v.get("volumes", {}).get(vtag)
                if not m:
                    continue
                if "error" in m:
                    print(
                        f"{r['name']:20} {v['settings_volume']:52} {'ERR':>6}  {m['error'][:40]}"
                    )
                    continue
                cold = m.get("read_full_cold")
                resid = (
                    f"{cold['worst_residency'] * 100:>5.1f}%" if cold else f"{'-':>6}"
                )
                print(
                    f"{r['name']:20} {v['settings_volume']:52} {m['ratio']:>6.1f} {m['bytes'] / 2**20:>8.2f} "
                    f"{_cell(m['write'], vol.nbytes):>15} {_cell(m['read_full_warm'], vol.nbytes):>15} "
                    f"{_cell(m['read_slice_warm'], vol.nbytes):>15} {_cell(cold, vol.nbytes):>15} {resid}"
                )

    for tag, cols in tables.items():
        raw = sum(c.nbytes for c in cols.values())
        print(f"\n## Table — {tag} fixture")
        if tag == "periodic":
            print(
                "#  adversarial for value-distribution codecs: exact periods 7 and 5, which"
            )
            print(
                "#  deflate's LZ77 window exploits and Pco/dictionary/bit-packing cannot (#497)."
            )
        else:
            print(
                "#  listmode-like: Poisson clock runs + continuous floats, shaped after real"
            )
            print(
                "#  /events_2p (#493). Both fixtures are always reported — one is choosing (#497)."
            )
        print(hdr.replace("slice", "  col"))
        for r in payload["results"].values():
            for vkey, v in r["variants"].items():
                m = v.get("tables", {}).get(tag)
                if not m:
                    continue
                if "error" in m:
                    print(
                        f"{r['name']:20} {v['settings_table']:52} {'ERR':>6}  {m['error'][:40]}"
                    )
                    continue
                cold = m.get("read_full_cold")
                resid = (
                    f"{cold['worst_residency'] * 100:>5.1f}%" if cold else f"{'-':>6}"
                )
                print(
                    f"{r['name']:20} {v['settings_table']:52} {m['ratio']:>6.1f} {m['bytes'] / 2**20:>8.2f} "
                    f"{_cell(m['write'], raw):>15} {_cell(m['read_full_warm'], raw):>15} "
                    f"{_cell(m['read_col_warm'], raw):>15} {_cell(cold, raw):>15} {resid}"
                )

    print("\n## Integrity — what each mechanism costs, and what it proves")
    print(
        "#  NOT like-for-like, and deliberately shown side by side anyway. fletcher32 and page\n"
        "#  CRCs are checked DURING a read, so their cost is the checked read vs the same read\n"
        "#  without them. tessera `verify` is a SEPARATE whole-file pass that re-derives every\n"
        "#  block digest against the sealed manifest.\n"
        "#  Reported as TIME (ms), not MB/s: `verify` hashes the STORED bytes, so MB/s over raw\n"
        "#  bytes would make a highly compressible file look absurdly fast (126 GB/s on the\n"
        "#  gradient, which is real and meaningless). Time is comparable across a read and a hash."
    )
    ihdr = (
        f"{'ecosystem':20} {'fixture':11} {'what is timed':44} "
        f"{'warm ms':>20} {'cold ms':>20} {'file MiB':>9}"
    )
    print(ihdr)
    datasets = [("volume", k, v.nbytes) for k, v in volumes.items()] + [
        ("table", k, sum(c.nbytes for c in cols.values())) for k, cols in tables.items()
    ]
    for r in payload["results"].values():
        ig = r.get("integrity")
        if not ig:
            continue
        chk = ig["variant"]
        for kind, fx, nbytes in datasets:
            src = "volumes" if kind == "volume" else "tables"
            m = r["variants"].get(chk, {}).get(src, {}).get(fx)
            if not m or "error" in m:
                continue
            if "verify_warm" in m:
                rows = [
                    (
                        "verify(): re-hash every block vs manifest",
                        m["verify_warm"],
                        m.get("verify_cold"),
                    )
                ]
            else:
                base = r["variants"].get("tuned", {}).get(src, {}).get(fx) or {}
                rows = [
                    (
                        "read WITHOUT checksum (tuned)",
                        base.get("read_full_warm"),
                        base.get("read_full_cold"),
                    ),
                    (
                        "read WITH checksum verified",
                        m.get("read_full_warm"),
                        m.get("read_full_cold"),
                    ),
                ]
            fmib = m["bytes"] / 2**20
            for what, w, c in rows:
                print(
                    f"{r['name']:20} {fx:11} {what:44} "
                    f"{_ms(w):>20} {_ms(c):>20} {fmib:>9.2f}"
                )
    print()
    for r in payload["results"].values():
        ig = r.get("integrity")
        if not ig:
            continue
        print(f"{r['name']}: {ig['mechanism']}")
        print(f"    detects : {ig['detects']}")
        print(f"    does NOT: {ig['does_not']}")
    print(
        "\nBoth a checksum and a sealed manifest catch a flipped bit. Only one answers\n"
        "'is this the artifact that was sealed, and by whom' — corruption detection is not\n"
        "whole-file tamper-evidence, and the table does not let one stand in for the other."
    )

    lone = [
        (r["name"], r["single_variant_reason"])
        for r in payload["results"].values()
        if r.get("single_variant_reason")
    ]
    if lone:
        print(
            "\n## Reported at ONE setting (no tuning lever exists — stated, not assumed)"
        )
        for nm, why in lone:
            print(f"{nm}: {why}")

    swmr = [r["name"] for r in payload["results"].values() if r["caps"].get("swmr")]
    print(f"\nSWMR / concurrent-reader support: {', '.join(swmr) if swmr else 'none'}")


def markdown(payload, volumes, tables) -> str:
    """Markdown tables for a PR body or docs, with the best cell per column bolded.

    The bolding is COMPUTED, never hand-placed. A hand-bolded table in this PR's first draft marked
    Tessera's continuous-table read as best while Parquet's was faster; generating the emphasis from
    the numbers removes that failure class rather than correcting one instance of it.
    Size: smallest is best. Throughput: highest MB/s is best (uncompressed formats will often win
    raw speed, and are bolded when they do).
    """
    env = payload["environment"]
    out = [
        f"_{env['cpu']} ×{env['cores']} · kernel {env['kernel']} · median of N={payload['iters']}, "
        "MB/s over raw bytes, `[lo-hi]` = slowest/fastest sample · best per column in **bold**_",
        "",
    ]

    def section(title, src, fixture, nbytes, partial_key, partial_label):
        rows = []
        for r in payload["results"].values():
            for v in r["variants"].values():
                m = v.get(src, {}).get(fixture)
                if not m or "error" in m:
                    continue
                settings = v.get(
                    "settings_volume" if src == "volumes" else "settings_table", ""
                )
                rows.append((r["name"], settings, m))
        if not rows:
            return
        cols = [
            ("MiB", lambda m: m["bytes"] / 2**20, min),
            ("write", lambda m: _mbps(nbytes, m["write"]["median"]), max),
            ("read", lambda m: _mbps(nbytes, m["read_full_warm"]["median"]), max),
            (partial_label, lambda m: _mbps(nbytes, m[partial_key]["median"]), max),
        ]
        if all("read_full_cold" in m for _n, _s, m in rows):
            cols.append(
                (
                    "cold read",
                    lambda m: _mbps(nbytes, m["read_full_cold"]["median"]),
                    max,
                )
            )
        best = {c: pick(f(m) for _n, _s, m in rows) for c, f, pick in cols}
        out.append(f"#### {title}")
        out.append("")
        out.append(
            "| ecosystem | settings | " + " | ".join(c for c, _f, _p in cols) + " |"
        )
        out.append("|---|---|" + "---:|" * len(cols))
        for n, st, m in rows:
            cells = []
            for c, f, _p in cols:
                val = f(m)
                txt = f"{val:.2f}" if c == "MiB" else f"{val:.0f}"
                cells.append(f"**{txt}**" if val == best[c] else txt)
            # A literal `|` in a settings string (e.g. "auto (pcodec|zstd, ...)") ends the cell
            # and silently shifts every column after it — escape it.
            st_md = st.replace("|", "\\|")
            out.append(f"| {n} | {st_md} | " + " | ".join(cells) + " |")
        out.append("")

    for k, vol in volumes.items():
        section(
            f"Volume — `{k}` fixture",
            "volumes",
            k,
            vol.nbytes,
            "read_slice_warm",
            "z-slice",
        )
    for k, cols in tables.items():
        raw = sum(c.nbytes for c in cols.values())
        section(f"Table — `{k}` fixture", "tables", k, raw, "read_col_warm", "1 column")
    return "\n".join(out) + "\n"


if __name__ == "__main__":
    main()
