---
type: issue
state: closed
created: 2026-02-25T22:31:51Z
updated: 2026-09-28T17:54:07Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/143
comments: 1
labels: area:testing, effort:large, area:core, spike
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-09-29T07:53:07.155Z
---

# [Issue 143]: [[SPIKE] Benchmark sequential read I/O and SWMR across fd5, HDF5, Zarr, NIfTI, DICOM, Parquet, and NeXus](https://github.com/vig-os/tessera/issues/143)

### Description

Benchmark sequential read I/O throughput and SWMR (Single Writer Multiple Reader) behavior for fd5 and the formats it competes with or complements. The goal is to produce reproducible numbers that inform the positioning document (#141) and identify performance bottlenecks in the fd5 read path.

### Problem Statement

fd5 adds conventions (embedded schema, content hashing, provenance, multiscale pyramids) on top of HDF5. These add value but may also add overhead. Users choosing between fd5 and alternatives need concrete answers to:

- How fast is sequential slice-by-slice read of a 3D volume in fd5 vs raw HDF5 vs Zarr vs NIfTI?
- What is the overhead of fd5's conventions (schema validation, attribute reads) on the read path?
- How do formats compare under concurrent read load (SWMR / multi-reader)?
- What is the read throughput for tabular/event data: fd5 compound datasets vs Parquet vs CSV?

### Proposed Solution

Create a benchmark suite that measures:

#### 1. Sequential read — volumetric (ND array)

| Scenario | Formats |
|----------|---------|
| Read full 3D volume into memory | fd5, raw HDF5, Zarr (v2), NIfTI (.nii), NIfTI compressed (.nii.gz), NeXus/HDF5 |
| Read single slice (axial) | fd5, raw HDF5, Zarr, NIfTI |
| Read slice from pyramid level | fd5 (pyramid), Zarr (NGFF multiscale) |
| Iterate all slices sequentially | fd5, raw HDF5, Zarr, NIfTI |
| Read 4D volume frame-by-frame | fd5, raw HDF5, Zarr |

Test volumes: 256x256x128 (small), 512x512x300 (typical CT), 512x512x600x20 (dynamic PET).

#### 2. Sequential read — tabular / event data

| Scenario | Formats |
|----------|---------|
| Read full event table | fd5 (compound dataset), Parquet, CSV |
| Column-selective read | fd5, Parquet |
| Row-range read | fd5, Parquet |

Test tables: 1M rows, 10M rows, 100M rows.

#### 3. SWMR / concurrent read

| Scenario | Formats |
|----------|---------|
| 1 writer + N readers (N=1,4,8) reading completed slices | fd5 (HDF5 SWMR mode), raw HDF5 SWMR |
| N concurrent readers, no writer | fd5, Zarr, NIfTI, Parquet |
| Read while another process writes to a different file in the same directory | Zarr (chunk files), DICOM (file-per-slice) |

HDF5 SWMR is a native feature; Zarr achieves concurrent reads via independent chunk files; NIfTI and DICOM have no built-in concurrency model.

#### 4. Metadata access

| Scenario | Formats |
|----------|---------|
| Read all attributes / header | fd5 (`h5dump -A` equivalent), NIfTI header, DICOM header, Zarr `.zattrs` |
| Schema introspection (time to understand file structure) | fd5 (`_schema` attr), Zarr (`.zarray` + `.zattrs`), NeXus (NXdata navigation) |

#### Measurements

For each scenario, report:
- **Wall-clock time** (median of 10 runs, after 3 warmup runs)
- **Throughput** (MB/s or rows/s)
- **Peak RSS** (memory)
- **File size on disk** (same data, each format)

#### Environment

- Benchmark on local SSD (no network I/O)
- Report OS, filesystem, Python version, library versions
- Use `pytest-benchmark` or `asv` (airspeed velocity) for reproducibility

### Alternatives Considered

- **Rely on published benchmarks**: Zarr and HDF5 have published comparisons, but none include fd5's convention overhead or the specific access patterns above.
- **Defer to users**: users will benchmark informally anyway; providing canonical numbers prevents misinformation and shows confidence.
- **Benchmark only fd5 internals**: already done in #90 (create/validate/hash). This issue specifically covers cross-format comparison and read-path performance.

### Additional Context

Results from this spike should feed into:
- #141 — positioning document (concrete numbers for the comparison table)
- #136 — read API design (identify hot paths that need optimization)
- #90 — complements existing create/validate/hash benchmarks with the read side

Relevant prior art and references:
- [Zarr vs HDF5 benchmarks (zarr-developers)](https://zarr.readthedocs.io/en/stable/)
- [HDF5 SWMR documentation](https://docs.hdfgroup.org/hdf5/develop/swmr.html)
- [Parquet vs HDF5 read performance (pandas docs)](https://pandas.pydata.org/docs/user_guide/io.html)
- [NeXus performance notes](https://manual.nexusformat.org/)
- [kerchunk — reference filesystem for HDF5/NetCDF as Zarr](https://github.com/fsspec/kerchunk)

### Impact

- Informs format positioning and user guidance
- Identifies fd5 read-path bottlenecks before v0.1 release
- Provides reproducible evidence for the "why fd5?" question

### Changelog Category

No changelog needed
---

# [Comment #1]() by [gerchowl]()

_Posted on September 28, 2026 at 05:54 PM_

Closing as **obsolete** (2026-09-28 backlog triage).

Evidence: fd5-era comparative bench. Tessera's benchmarking is scoped by #388 (`tessera bench compare` vs Parquet/HDF5); the SWMR angle is not on the current Tessera roadmap.

https://claude.ai/code/session_01XdERKMVDAwfMJSKdTytNnK

