---
type: issue
state: open
created: 2026-11-11T01:35:21Z
updated: 2026-11-11T03:36:39Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/822
comments: 1
labels: feature, priority:medium, area:core
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-11T08:50:31.934Z
---

# [Issue 822]: [perf(format): list-mode .tsra is +16 % vs GE's as-delivered codec and +9 % vs tuned HDF5 on Siemens — beatable with an archival profile? (fixtures attached)](https://github.com/vig-os/tessera/issues/822)

## Description

On real vendor list-mode, `.tsra` is **larger** than both the vendors' as-delivered formats (GE) and tuned open HDF5 (GE and Siemens). The vendor files are **cold-tier** products: compressed once, after acquisition, with geometry-aware or hand-packed codecs. `.tsra` was written with the **hot/live** settings (fixed zstd-3, default Pcodec, no sort-aware encoding). The fair question is therefore whether an archival profile (#796) can beat the vendor formats, and this issue provides the fixtures to test exactly that.

## Measurements (tessera `6a3078079`, 2²⁴-row blocks, bit-exact round trip in every row)

**GE Discovery MI Gen2, one bed, 191,575,099 coincidences** (decoded `/MiceList/TofCoinc`: 5 × int16 `lo_ax, lo_tx, hi_ax, hi_tx, delta_t`; no per-event energy):

| Format | B/event | vs GE as delivered |
|---|---:|---:|
| GE as delivered (closed GLEPL_MI HDF5 filter 32783, geometry-aware; needs GE's binary plugin to read) | **3.364** | 1.00 |
| HDF5 columnar 1-D, shuffle + zstd-3, 1 Mi-row chunks | 3.768 | +12 % |
| `.tsra` events product | **3.899** | **+16 %** |
| Parquet zstd-19 | 4.265 | +27 % |
| Decoded, uncompressed | 10.000 | ×3.0 |

**Siemens Biograph Vision, 55,004,119 prompts + delayeds** (64-bit packets decoded to 11 columns: `ms u4, prompt u1, tof i2, ax ay ae bx by be xe u1, rsvd u1`):

| Format | B/event | vs native |
|---|---:|---:|
| Native packets | 8.004 | 1.00 |
| Native + zstd-19 | 7.624 | −5 % |
| HDF5 shuffle + zstd-19 | **6.902** | **−14 %** |
| Parquet zstd-19, 1 Mi-row groups | 7.211 | −10 % |
| `.tsra` | **7.508** | **−6 %** (~9 % larger than tuned HDF5) |

Secondary data point (no fixture, only on the source host): a Siemens `PET_CALIBRATION` float32 norm vector (94.3 MB) shrinks to 25.6 MB as a `.tsra` array (codec auto) versus 11.7 MB with HDF5 shuffle + zstd-19, so `.tsra` is 2.2× larger.

## Questions for the architect

1. Is the gap structural or just a matter of settings? Candidates, all lossless and deterministic:
   - zstd level, or the byte codec per column, for small-cardinality int16 / u1 columns;
   - delta or frame-of-reference coding of the near-monotonic time column (`t_ms` / `ms`);
   - a byte-shuffle-like transform before the byte codec. HDF5's shuffle is what wins on Siemens;
   - block or row-group size;
   - joint encoding of crystal pairs (`lo_*`/`hi_*`, `a*`/`b*`), which is what GLEPL presumably exploits through scanner geometry. Not verified: the plugin was not disassembled.
2. Can an archival profile (#796) reach **≤ 3.36 B/event on GE** and **≤ 6.90 B/event on Siemens** while keeping the ADR-0024 determinism?

## Fixtures

**Internal fixture set `tessera-vendor-2026-11`** (977 MB; `MANIFEST.sha256` sha256 `dfd9c1e6…ebf2d`). It holds real de-identified veterinary scans, so it is **not public**: ask the maintainer for the location.

- `ge/events/*.npy.zst`: 54,161,949 GE events (≈34 s of the bed above; the same 5 × int16 + `t_ms` u4).
- `siemens/events/*.npy.zst`: the 55,004,119-event Siemens slice above as 11 decoded columns.
- `siemens/lm/*.ptd`: the same slice as native packets.
- `README.md`: provenance, schema and the reference B/event on each slice.

Decode with `zstd -d` and `numpy.load`. Bytes/event = stored bytes / events.

## Related

#796 (archival re-encode), #628 (string-heavy tables ~3× larger than Parquet: a similar "normalise forbids a better encoding" class), ADR-0024 (ALP excluded; on this data ALP potential = 0 %, see the #796 comment).

## Changelog Category

Changed

---

# [Comment #1]() by [gerchowl]()

_Posted on November 11, 2026 at 03:36 AM_

Measured follow-up in #828: a per-column codec sweep on the same fixtures, with every round trip verified. Plain Pco per column gives 3.18 B/ev on GE (`.tsra` 3.91) and 6.84 B/ev on Siemens (`.tsra` 7.51). The best codec per column gives 3.09 and 6.83; Siemens is then at its order-0 entropy. A fast fixed profile gives 3.15 / 6.89 B/ev at ≈25 / 49 M ev/s single-core codec time (indicative). Row-group size is not the cause (+0.2–0.3 %), so the gap points at scheme selection.

