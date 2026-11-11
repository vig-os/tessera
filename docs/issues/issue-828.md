---
type: issue
state: open
created: 2026-11-11T03:36:30Z
updated: 2026-11-11T03:43:36Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/828
comments: 1
labels: feature, priority:high, area:core
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-11T08:50:28.545Z
---

# [Issue 828]: [perf(format): per-column codec profiles — measured sweep: plain Pco per column is 19 % smaller than .tsra on GE list-mode; a fixed fast profile beats .tsra on size and per-core speed](https://github.com/vig-os/tessera/issues/828)

## Description

A per-column codec sweep on the #822 list-mode fixtures shows two things:

- **Plain Pcodec on every column is 19 % smaller than `.tsra` on GE and 9 % smaller on Siemens**, and about 3× faster on one core.
- **A fixed per-column profile beats `.tsra` and tuned HDF5 on size, and on GE probably the vendor's closed codec too, while encoding 4–8× faster per core.**

`.tsra` stores every column separately, so it can carry a per-column codec. An HDF5 compound dataset cannot: it has one filter for all fields. Today, though, tessera's table codecs are fixed in code: BtrBlocks plus Pco (`tessera-io/src/table.rs:925-940`), with no codec field for tables and no profile.

This issue proposes a **fixed, measured codec profile per product type**. It is deterministic and needs no per-block scheme sampling. It is the concrete form of #822 (size) and #826 (writer speed), and its dense variant feeds #796 (archival re-encode).

## Setup

- **Fixtures** from #822:
  - GE Discovery MI Gen2, 54,161,949 events;
  - Siemens Biograph Vision, 55,004,119 events (11 columns).
- **Codecs:** every column is compressed in 2²²-row blocks with zstd 1/3/9/19, byte-shuffle + zstd 3/9, and Pco levels 4/8/12. Each runs on the column as stored and with integers narrowed to the smallest lossless dtype.
- **Lossless derived columns** are also tried: the delta of the time column, and the B-crystal minus A-crystal coordinate (reconstructable from the A column).
- **Every block's round trip was verified: 312/312 OK.**
- **Tools:** pcodec 1.1.0 and zstandard 0.25.0 through their Python bindings, on one pinned core.
- **Speeds are indicative.** The host was shared (load ≈ 4.7) and only codec time is counted, with no hashing or framing. Compare speeds within the table, not with the bench writer.
- **References:** the bench's own writer outputs on the same slices.

### GE — 54,161,949 events

| Column | Best (source columns only) | B/ev | Best with transforms | B/ev | Fast (≥250 MB/s) with transforms | B/ev | Order-0 entropy B/ev |
|---|---|---:|---|---:|---|---:|---:|
| lo_ax | narrow->uint8 · zstd-19 | 0.413 | lo_ax: narrow->uint8 · zstd-19 | 0.413 | lo_ax: as-is · pco-12 | 0.444 | 0.718 (distinct=54 range=[0,53]) |
| lo_tx | as-is · pco-12 | 1.064 | lo_tx: as-is · pco-12 | 1.064 | lo_tx: as-is · pco-4 | 1.082 | 1.063 (distinct=479 range=[0,478]) |
| hi_ax | as-is · pco-12 | 0.070 | hi_ax: as-is · pco-12 | 0.070 | hi_ax: as-is · pco-12 | 0.070 | 0.717 (distinct=54 range=[0,53]) |
| hi_tx | as-is · pco-12 | 1.059 | hi_tx-lo_tx: as-is · pco-8(default) | 1.000 | hi_tx-lo_tx: as-is · pco-4 | 1.021 | 0.999 (distinct=415 range=[65,479]) |
| delta_t | as-is · pco-8(default) | 0.537 | delta_t: as-is · pco-8(default) | 0.537 | delta_t: as-is · pco-8(default) | 0.537 | 0.536 (distinct=31 range=[-195,195]) |
| t_ms | narrow->uint16 · pco-12 | 0.001 | d(t_ms): as-is · pco-12 | 0.001 | d(t_ms): as-is · pco-12 | 0.001 | 0.001 (distinct=3 range=[0,12656]) |

| Profile | B/event | Total MB | Single-core encode (indicative) |
|---|---:|---:|---:|
| reference: .tsra default (bench writer) | 3.907 | 211.6 | — |
| reference: HDF5 shuffle+zstd (bench writer) | 3.742 | 202.7 | — |
| **per-column best, source columns** | **3.143** | 170.3 | 3.6 M ev/s |
| **per-column best + lossless transforms** | **3.085** | 167.1 | 3.7 M ev/s |
| **fast per-column (≥250 MB/s/col) + transforms** | **3.154** | 170.8 | 25.2 M ev/s |
| uniform zstd-3 (no transforms) | 4.075 | 220.7 | 34.2 M ev/s |
| uniform pco-8(default) (no transforms) | 3.182 | 172.4 | 22.3 M ev/s |
| uniform shuf+zstd-3 (no transforms) | 3.742 | 202.7 | 52.9 M ev/s |

### SIEMENS — 55,004,119 events

| Column | Best (source columns only) | B/ev | Best with transforms | B/ev | Fast (≥250 MB/s) with transforms | B/ev | Order-0 entropy B/ev |
|---|---|---:|---|---:|---|---:|---:|
| ms | narrow->uint16 · pco-12 | 0.000 | d(ms): as-is · pco-12 | 0.000 | d(ms): as-is · pco-12 | 0.000 | 0.000 (distinct=2 range=[0,1]) |
| prompt | as-is · pco-8(default) | 0.106 | prompt: as-is · pco-8(default) | 0.106 | prompt: as-is · pco-8(default) | 0.106 | 0.106 (distinct=2 range=[0,1]) |
| tof | as-is · pco-8(default) | 1.097 | tof: as-is · pco-8(default) | 1.097 | tof: as-is · pco-4 | 1.119 | 1.096 (distinct=511 range=[-255,255]) |
| ax | as-is · pco-4 | 0.666 | ax: as-is · pco-4 | 0.666 | ax: as-is · zstd-1 | 0.673 | 0.665 (distinct=40 range=[0,39]) |
| ay | as-is · pco-12 | 0.791 | ay: as-is · pco-12 | 0.791 | ay: as-is · zstd-1 | 0.796 | 0.790 (distinct=80 range=[0,79]) |
| ae | as-is · zstd-19 | 0.919 | ae: as-is · zstd-19 | 0.919 | ae: as-is · zstd-9 | 0.923 | 0.919 (distinct=214 range=[42,255]) |
| bx | as-is · pco-4 | 0.666 | bx: as-is · pco-4 | 0.666 | bx: as-is · zstd-1 | 0.673 | 0.665 (distinct=40 range=[0,39]) |
| by | as-is · pco-12 | 0.791 | by: as-is · pco-12 | 0.791 | by: as-is · zstd-1 | 0.796 | 0.790 (distinct=80 range=[0,79]) |
| be | as-is · zstd-19 | 0.921 | be: as-is · zstd-19 | 0.921 | be: as-is · zstd-9 | 0.925 | 0.921 (distinct=214 range=[42,255]) |
| xe | as-is · zstd-19 | 0.872 | xe: as-is · zstd-19 | 0.872 | xe: as-is · zstd-9 | 0.880 | 0.879 (distinct=133 range=[1,133]) |
| rsvd | as-is · zstd-9 | 0.000 | rsvd: as-is · zstd-9 | 0.000 | rsvd: as-is · zstd-9 | 0.000 | -0.000 (distinct=1 range=[0,0]) |

| Profile | B/event | Total MB | Single-core encode (indicative) |
|---|---:|---:|---:|
| reference: .tsra default (bench writer) | 7.509 | 413.0 | — |
| reference: HDF5 shuffle+zstd (bench writer) | 7.034 | 386.9 | — |
| reference: HDF5 shuffle+zstd-19 (§4.3) | 6.902 | 379.6 | — |
| reference: native 64-bit packets | 8.004 | 440.3 | — |
| **per-column best, source columns** | **6.828** | 375.6 | 4.8 M ev/s |
| **per-column best + lossless transforms** | **6.828** | 375.6 | 4.8 M ev/s |
| **fast per-column (≥250 MB/s/col) + transforms** | **6.891** | 379.0 | 49.2 M ev/s |
| uniform zstd-3 (no transforms) | 7.197 | 395.8 | 66.2 M ev/s |
| uniform pco-8(default) (no transforms) | 6.843 | 376.4 | 14.1 M ev/s |
| uniform shuf+zstd-3 (no transforms) | 7.037 | 387.1 | 89.6 M ev/s |

## Findings

1. **The gap to `.tsra` comes neither from Pco nor from the row-group size.**
   - Plain Pco-8 per column: 3.18 bytes/event on GE (`.tsra` 3.91) and 6.84 on Siemens (`.tsra` 7.51).
   - Re-running at tessera's `ROWS_PER_GROUP` = 65,536 rows changes Pco by only +0.2 % (GE) and +0.3 % (Siemens), and zstd by up to +2.9 %.
   - So the excess most likely comes from **which schemes BtrBlocks picks for these integer columns**, and from what the block layout adds. Please confirm by logging the chosen scheme per column and block on the fixture.
2. **Best per-column codec, GE: 3.09 bytes/event.**
   - That is 21 % below `.tsra` and 18 % below HDF5 shuffle + zstd (3.74).
   - It is also below GE's closed, geometry-aware GLEPL codec at 3.36. Caveat: the 3.36 is for the full bed of 191 M events; GLEPL was not measured on the slice.
   - The slice is representative: `.tsra` itself gives 3.899 on the full bed and 3.907 on the slice.
3. **Best per-column codec, Siemens: 6.83 bytes/event.**
   - That is 9 % below `.tsra` and below HDF5 shuffle + zstd-19 (6.90).
   - It equals the summed **order-0 entropy** of the columns, so coding each column independently has reached its limit on Siemens. Further gains need joint, geometry-aware modelling.
4. **A fast profile costs almost nothing in size.** With every column restricted to codecs that encode at ≥ 250 MB/s on one core:
   - GE: 3.15 bytes/event at ≈ 25 M events/s of single-core codec time;
   - Siemens: 6.89 bytes/event at ≈ 49 M events/s;
   - for comparison, the whole `.tsra` writer measured 6.1–6.7 M events/s per core (#826).
5. **Transforms:**
   - storing `hi_tx − lo_tx` instead of `hi_tx` saves 5.6 % on that GE column;
   - delta-coding time makes no difference, because Pco already takes both time columns to ≈ 0 bytes/event;
   - the Siemens B − A coordinate differences don't help.
6. **The event stream has sequential structure.** GE `hi_ax` codes at 0.070 bytes/event with Pco, against an order-0 entropy of 0.717. The stream has long runs. The codec choice should exploit them, and an archival sort should not destroy them.

## Proposed profiles, fixed per product type

**GE list-mode, fast/live:**
- `lo_ax` Pco-12, `lo_tx` Pco-4, `hi_ax` Pco-12;
- `hi_tx` stored as `hi_tx − lo_tx`, Pco-4;
- `delta_t` Pco-8;
- `t_ms` delta, Pco-12.

**GE dense/archival (#796):** the same, except `lo_ax` narrowed to u8 with zstd-19, `lo_tx` Pco-12, and `hi_tx − lo_tx` Pco-8.

**Siemens list-mode, fast/live:**
- `ms` delta, Pco-12;
- `prompt` Pco-8, `tof` Pco-4;
- `ax ay bx by` zstd-1;
- `ae be xe rsvd` zstd-9.

**Siemens dense/archival:**
- `tof` Pco-8;
- `ax bx` Pco-4, `ay by` Pco-12;
- `ae be xe` zstd-19.

**Requirements:**
- the profile id and each column's codec are recorded in the manifest, so they are part of the seal;
- the encoders are deterministic: Pco and zstd are integer codecs here, so the ADR-0024 float concern does not arise;
- no scheme sampling happens at write time.

## Questions for the architect

1. Which schemes does BtrBlocks choose on these columns today? Does forcing Pco, or the profile above, reproduce these numbers inside `.tsra`?
2. Where should a profile live: in the product type (schema), as a writer option recorded in the manifest, or both?
3. Can the fast profile plus fully used encode workers close #826? The target is at least raw → `zstd -T`, ≈ 49–57 M events/s at 8 cores.

## Fixtures and code

- **Fixtures:** the internal set from #822. It is not public; ask the maintainer.
- **Per-row results:** `sweep.csv` (312 rows) and `sweep_rg65536.csv` (the 65,536-row check), available on request.

<details><summary>codec_sweep.py</summary>

```python
"""Per-column codec sweep on the tessera-vendor-2026-11 list-mode fixtures.

For every column (and a few lossless derived columns), compress in 2^22-row blocks (tessera BLOCK_ROWS)
with each candidate, verify the round trip, and record bytes and single-thread encode speed.
Writes <out>/sweep.csv and <out>/profiles.json. Ratios are exact; speeds are indicative (shared host).
"""
import io, json, math, sys, time, csv
import numpy as np, zstandard as zstd, pcodec
from pcodec import standalone, ChunkConfig

FIX, OUT = sys.argv[1], sys.argv[2]
import os
BLOCK = int(os.environ.get('BLOCK', 1 << 22))

def load(vendor, name):
    with open(f"{FIX}/{vendor}/events/{name}.npy.zst", "rb") as f:
        raw = zstd.ZstdDecompressor().stream_reader(f).read()
    return np.load(io.BytesIO(raw))

def blocks(a):
    for i in range(0, len(a), BLOCK):
        yield a[i:i + BLOCK]

def shuffle(b, itemsize):
    return np.frombuffer(b, np.uint8).reshape(-1, itemsize).T.copy().tobytes()

def unshuffle(b, itemsize):
    return np.frombuffer(b, np.uint8).reshape(itemsize, -1).T.copy().tobytes()

def narrow(a):
    lo, hi = int(a.min()), int(a.max())
    for dt in (np.uint8, np.int8, np.uint16, np.int16, np.uint32, np.int32):
        info = np.iinfo(dt)
        if info.min <= lo and hi <= info.max:
            return a.astype(dt)
    return a

PCO_OK = {np.dtype(t) for t in ("u2", "i2", "u4", "i4", "u8", "i8", "f2", "f4", "f8")}
def pco_ready(a):
    # pcodec has no 8-bit types; widen losslessly
    if a.dtype in PCO_OK: return a
    return a.astype(np.int16 if a.dtype.kind == "i" else np.uint16)

def cand_zstd(level, shuf=False):
    def enc(a):
        b = a.tobytes(); b = shuffle(b, a.itemsize) if shuf and a.itemsize > 1 else b
        return zstd.ZstdCompressor(level=level).compress(b)
    def dec(c, a):
        b = zstd.ZstdDecompressor().decompress(c)
        b = unshuffle(b, a.itemsize) if shuf and a.itemsize > 1 else b
        return np.frombuffer(b, a.dtype)
    return enc, dec

def cand_pco(level):
    def enc(a):
        return standalone.simple_compress(pco_ready(a), ChunkConfig(compression_level=level))
    def dec(c, a):
        return standalone.simple_decompress(c).astype(a.dtype)
    return enc, dec

CANDS = {"shuf+zstd-3": cand_zstd(3, True), "zstd-3": cand_zstd(3), "pco-8(default)": cand_pco(8)} if os.environ.get("SMALL") else {
    "zstd-1": cand_zstd(1), "zstd-3": cand_zstd(3), "zstd-9": cand_zstd(9), "zstd-19": cand_zstd(19),
    "shuf+zstd-3": cand_zstd(3, True), "shuf+zstd-9": cand_zstd(9, True),
    "pco-4": cand_pco(4), "pco-8(default)": cand_pco(8), "pco-12": cand_pco(12),
}

def analytic(a):
    lo, hi = int(a.min()), int(a.max())
    vals, counts = np.unique(a, return_counts=True)
    p = counts / counts.sum()
    H = float(-(p * np.log2(p)).sum())
    return {"for_bitpack_bits": max(1, math.ceil(math.log2(hi - lo + 1))) if hi > lo else 0,
            "dict_bits": max(1, math.ceil(math.log2(len(vals)))) if len(vals) > 1 else 0,
            "distinct": int(len(vals)), "entropy_bits": H, "min": lo, "max": hi}

def sweep(vendor, label, a, rows, kind):
    n = len(a)
    an = analytic(a)
    for k, bits in (("analytic:FOR+bitpack", an["for_bitpack_bits"]), ("analytic:dict+bitpack", an["dict_bits"]),
                    ("analytic:order0-entropy", an["entropy_bits"])):
        rows.append(dict(vendor=vendor, column=label, kind=kind, dtype=str(a.dtype), candidate=k,
                         bytes=int(math.ceil(n * bits / 8)), b_per_event=bits / 8, enc_mb_s="", ok="analytic",
                         note=f"distinct={an['distinct']} range=[{an['min']},{an['max']}]"))
    variants = [("as-is", a)]
    if os.environ.get("SMALL"): variants = variants[:1]; rows_skip = True
    na = narrow(a)
    if na.dtype != a.dtype: variants.append((f"narrow->{na.dtype}", na))
    for vname, v in variants:
        for cname, (enc, dec) in CANDS.items():
            if cname.startswith("shuf") and v.itemsize == 1: continue
            tot, t = 0, 0.0; ok = True
            for blk in blocks(v):
                blk = np.ascontiguousarray(blk)
                t0 = time.perf_counter(); c = enc(blk); t += time.perf_counter() - t0
                tot += len(c)
                if not np.array_equal(dec(c, blk), blk): ok = False
            raw_mb = n * a.itemsize / 1e6
            rows.append(dict(vendor=vendor, column=label, kind=kind, dtype=str(v.dtype), candidate=f"{vname} | {cname}",
                             bytes=tot, b_per_event=tot / n, enc_mb_s=round(raw_mb / t, 1), ok=ok, note=""))
            print(f"{vendor:8s} {label:14s} {vname:16s} {cname:16s} {tot/n:7.4f} B/ev {raw_mb/t:8.1f} MB/s ok={ok}", flush=True)

def delta(a):
    d = np.empty(len(a), np.int64); d[0] = a[0]; d[1:] = np.diff(a.astype(np.int64))
    return narrow(d)

SCHEMA = {
    "ge": {"cols": ["lo_ax", "lo_tx", "hi_ax", "hi_tx", "delta_t", "t_ms"], "time": "t_ms",
           "pairs": [("hi_tx", "lo_tx"), ("hi_ax", "lo_ax")]},
    "siemens": {"cols": ["ms", "prompt", "tof", "ax", "ay", "ae", "bx", "by", "be", "xe", "rsvd"], "time": "ms",
                "pairs": [("bx", "ax"), ("by", "ay"), ("be", "ae")]},
}

rows = []
for vendor, s in SCHEMA.items():
    cols = {c: load(vendor, c) for c in s["cols"]}
    n = len(next(iter(cols.values())))
    for c, a in cols.items():
        sweep(vendor, c, a, rows, "source")
    if os.environ.get("SMALL"): del cols; continue
    sweep(vendor, f"d({s['time']})", delta(cols[s["time"]]), rows, f"derived: delta of {s['time']} (replaces it)")
    for hi, lo in s["pairs"]:
        d = narrow(cols[hi].astype(np.int32) - cols[lo].astype(np.int32))
        sweep(vendor, f"{hi}-{lo}", d, rows, f"derived: replaces {hi} (lossless given {lo})")
    del cols

with open(f"{OUT}/sweep{os.environ.get('TAG','')}.csv", "w", newline="") as f:
    w = csv.DictWriter(f, fieldnames=list(rows[0].keys())); w.writeheader(); w.writerows(rows)
print("wrote", f"{OUT}/sweep.csv", len(rows), "rows")
```
</details>

<details><summary>codec_profiles.py</summary>

```python
"""Build per-column codec profiles from sweep.csv and print markdown tables."""
import csv, sys, collections, json

rows = list(csv.DictReader(open(sys.argv[1])))
N = {"ge": 54161949, "siemens": 55004119}
ITEM = {"int8": 1, "uint8": 1, "int16": 2, "uint16": 2, "int32": 4, "uint32": 4, "int64": 8}
# references on the SAME slices (tessera-bench RESULTS §5 writer table / §4.3)
REF = {"ge": [(".tsra default (bench writer)", 211.6e6), ("HDF5 shuffle+zstd (bench writer)", 202.7e6)],
       "siemens": [(".tsra default (bench writer)", 413.0e6), ("HDF5 shuffle+zstd (bench writer)", 386.9e6),
                   ("HDF5 shuffle+zstd-19 (§4.3)", 379_642_225), ("native 64-bit packets", 440_252_728)]}
BASE = {"ge": ["lo_ax", "lo_tx", "hi_ax", "hi_tx", "delta_t", "t_ms"],
        "siemens": ["ms", "prompt", "tof", "ax", "ay", "ae", "bx", "by", "be", "xe", "rsvd"]}
REPL = {"ge": {"t_ms": ["d(t_ms)"], "hi_tx": ["hi_tx-lo_tx"], "hi_ax": ["hi_ax-lo_ax"]},
        "siemens": {"ms": ["d(ms)"], "bx": ["bx-ax"], "by": ["by-ay"], "be": ["be-ae"]}}

by = collections.defaultdict(list)
asis_item = {}
for r in rows:
    key = (r["vendor"], r["column"])
    if r["ok"] == "analytic":
        by[key].append(r); continue
    if r["ok"] != "True": continue
    if r["candidate"].startswith("as-is"): asis_item[key] = ITEM[r["dtype"]]
    by[key].append(r)

def enc_s(r, v):
    n = N[v]; return n * asis_item[(v, r["column"])] / (float(r["enc_mb_s"]) * 1e6)

def best(v, col, min_speed=0.0):
    c = [r for r in by[(v, col)] if r["ok"] == "True" and float(r["enc_mb_s"]) >= min_speed]
    return min(c, key=lambda r: int(r["bytes"]))

def entropy(v, col):
    return [r for r in by[(v, col)] if r["candidate"] == "analytic:order0-entropy"][0]

def profile(v, derived, min_speed):
    picks = []
    for col in BASE[v]:
        opts = [col] + (REPL[v].get(col, []) if derived else [])
        cand = [best(v, o, min_speed) for o in opts]
        picks.append(min(cand, key=lambda r: int(r["bytes"])))
    tot = sum(int(r["bytes"]) for r in picks); t = sum(enc_s(r, v) for r in picks)
    return picks, tot, t

out = {}
for v in ("ge", "siemens"):
    n = N[v]
    print(f"\n### {v.upper()} — {n:,} events\n")
    print("| Column | Best (source columns only) | B/ev | Best with transforms | B/ev | Fast (≥250 MB/s) with transforms | B/ev | Order-0 entropy B/ev |")
    print("|---|---|---:|---|---:|---|---:|---:|")
    pA, tA, sA = profile(v, False, 0); pB, tB, sB = profile(v, True, 0); pC, tC, sC = profile(v, True, 250)
    for i, col in enumerate(BASE[v]):
        a, b, c = pA[i], pB[i], pC[i]
        e = entropy(v, b["column"])
        print(f"| {col} | {a['candidate'].replace(' | ',' · ')} | {float(a['b_per_event']):.3f} | {b['column']}: {b['candidate'].replace(' | ',' · ')} | {float(b['b_per_event']):.3f} | {c['column']}: {c['candidate'].replace(' | ',' · ')} | {float(c['b_per_event']):.3f} | {float(e['b_per_event']):.3f} ({e['note']}) |")
    print()
    print("| Profile | B/event | Total MB | Single-core encode (indicative) |")
    print("|---|---:|---:|---:|")
    for name, tot in REF[v]:
        print(f"| reference: {name} | {tot/n:.3f} | {tot/1e6:.1f} | — |")
    for name, tot, t in (("per-column best, source columns", tA, sA), ("per-column best + lossless transforms", tB, sB),
                         ("fast per-column (≥250 MB/s/col) + transforms", tC, sC)):
        print(f"| **{name}** | **{tot/n:.3f}** | {tot/1e6:.1f} | {n/t/1e6:.1f} M ev/s |")
    for uni in ("as-is | zstd-3", "as-is | pco-8(default)", "as-is | shuf+zstd-3"):
        tot = 0; t = 0
        for col in BASE[v]:
            r = [x for x in by[(v, col)] if x["candidate"] == uni] or [x for x in by[(v, col)] if x["candidate"] == "as-is | zstd-3"]
            tot += int(r[0]["bytes"]); t += enc_s(r[0], v)
        print(f"| uniform {uni.split(' | ')[1]} (no transforms) | {tot/n:.3f} | {tot/1e6:.1f} | {n/t/1e6:.1f} M ev/s |")
    out[v] = {"dense": [(r["column"], r["candidate"]) for r in pB], "fast": [(r["column"], r["candidate"]) for r in pC]}
json.dump(out, open(sys.argv[2], "w"), indent=1)
```
</details>

## Changelog Category

Changed

---

# [Comment #1]() by [gerchowl]()

_Posted on November 11, 2026 at 03:43 AM_

**Is the gap metadata? No.** Both sides cover the same columns: GE 5 × int16 + `t_ms` u4 = 14 B/ev raw, and Siemens 11 columns = 15 B/ev raw, matching the bench writer's 758 / 825 MB inputs.

Zip-entry breakdown of the real `.tsra` files:

| File | Data block entries | Manifest + provenance + mimetype | Outside blocks |
|---|---:|---:|---:|
| Siemens 55 M events, `tsrabench_default` (413.2 MB) | 413,186,168 B (14 blocks) | 19.3 kB | 0.005 % |
| GE bed 0, 191.6 M events (747.4 MB) | 747,365,312 B (46 blocks) | 38.0 kB | 0.005 % |

The in-block Vortex statistics (zone maps every 8,192 rows) and the footer aren't broken out in the manifest. An upper bound: 6.7 k zones × 11 columns × ≤ 6 stats × 8 B ≈ 3.5 MB, about 1 % or less of the file. That can't account for a 9–19 % gap, so the excess is in the column encodings themselves.

**Ask:** report per-column encoded bytes and per-column stats/layout bytes from one block, e.g. via the Vortex layout tree, so the encoding share and the metadata share can be separated exactly.

