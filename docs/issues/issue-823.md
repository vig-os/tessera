---
type: issue
state: open
created: 2026-11-11T01:35:23Z
updated: 2026-11-11T03:04:41Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/823
comments: 1
labels: feature, priority:medium, area:io
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-11T08:50:31.380Z
---

# [Issue 823]: [perf(io): reader reads 4.6–5× more bytes than vendor HDF5 on column/time-range tasks — whole-block read + re-hash per column()](https://github.com/vig-os/tessera/issues/823)

## Description

On column-subset and time-range tasks, `.tsra` reads **4.6–5× more bytes** than GE's as-delivered HDF5 list-mode read through GE's plugin. On volumes, it reads the whole blob for any slice. The cause is in the reader, not the format: there is no column-projected I/O, and each `column()` call reads and re-hashes the whole all-column block.

## Measurements (tessera `6a3078079`; logical bytes = `rchar`; the result sha256 is identical for both formats in every task)

GE Discovery MI Gen2, one bed, 191,575,099 events (5 int16 columns):

| Task | Reader | Bytes read | Peak RSS |
|---|---|---:|---:|
| T2: 10 s time slice (15,924,921 rows) | GE HDF5 + plugin (row range via TimeMarker; 2 chunks) | **82.8 MB** | 381 MB |
| T2 | `.tsra` (row range via `time_markers`; overlapping 2²² blocks) | **410.2 MB** (5.0×) | 741 MB |
| T3: per-crystal counts, 4 of 5 columns, all rows | GE HDF5 + plugin (24 chunks of the 4 columns) | **645.8 MB** | 304 MB |
| T3 | `.tsra` (4 `column()` calls per block) | **2,989.6 MB** (4.6×) | 186 MB |

Volumes (public TCIA ACRIN-NSCLC-FDG-PET, DOI 10.7937/tcia.2019.30ilqfcl, GE PT 534 × 128 × 128 int16):
- `tessera slice` reads the whole 2.33 MB `blocks/volume` STORED entry for an axial slice, a coronal slice, a sagittal slice or a 64³ ROI. One DICOM file is 36 KB for an axial slice.
- Coronal MIP on the CT (534 × 512 × 512): `.tsra` peak RSS is **749 MB** versus 314 MB for pydicom, because the CLI decodes the full volume.

## Root cause (code refs at `6a3078079`)

- `crates/tessera-io/src/container.rs:469-490`: `read_block` does `read_to_end` on the whole block entry and re-hashes it against the manifest digest.
- `crates/tessera-py/src/lib.rs:118`: `Reader.column(block, col)` calls `read_block`, then decodes one column. So each column call pays the full block read plus a full re-hash, and T3 on 4 of 5 columns reads the events product ~4×.
- **Corrected:** `Reader::open` is *not* a cause. It reads the zip central directory, `mimetype` and `manifest.json` and verifies the manifest's three hashes without touching payloads (`container.rs:333-417`); open takes ≈ 4 ms measured. The doc comment at `container.rs:332` says "full seal verification", which is misleading and worth fixing.
- ADR-0062 (streaming table provider, Accepted) is not implemented: no `scan` module, no projection or predicate push-down.
- Volumes: the ADR-0023 single-blob payload has no chunk-level range read (ADR-0027 / ADR-0028 direction).

## Proposed direction

1. Column-projected block reads: read and hash only the column segments requested. This needs per-column digests or a Merkle sub-tree per block.
2. Keep "verified before use" for every returned byte, but verify only the segments that were read (per-column digests), not the whole block.
3. Cache decoded blocks across `column()` calls within one reader.
4. Chunk-range reads for volumes.

Target: T2 ≤ 83 MB and T3 ≤ 646 MB on the fixture, with an identical result hash.

## Fixtures

- List-mode: the internal fixture set `tessera-vendor-2026-11` (see #822; not public, ask the maintainer). `ge/events` holds 54.16 M events of the same bed.
- Volumes: the public TCIA series above.

## Related

#777 (remote range-read and projection), #805 (C++ read side), #654 / #666 (streaming DataFusion provider), #347 (stats from the chunk index without a full decode), closed #212 (Python column projection).

## Changelog Category

Changed

---

# [Comment #1]() by [gerchowl]()

_Posted on November 11, 2026 at 03:04 AM_

Correction to the original text: `Reader::open` is not a cause. It verifies only the manifest's three hashes and touches no payload (`container.rs:333-417`; open takes ≈ 4 ms, measured). The doc comment at `container.rs:332` ("full seal verification") is misleading. All of the extra bytes come from `read_block` reading and re-hashing the whole all-column block on every `column()` call. The body is updated accordingly.

