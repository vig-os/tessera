---
type: issue
state: closed
created: 2026-02-25T20:24:14Z
updated: 2026-09-28T17:53:22Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/108
comments: 1
labels: feature, epic
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-09-29T07:53:13.495Z
---

# [Issue 108]: [[EPIC] Phase 6: Ingest Layer (fd5.ingest)](https://github.com/vig-os/tessera/issues/108)

## Overview

Add a **Layer 3 ingest module** (`fd5.ingest`) that converts external data formats (DICOM, NIfTI, MIDAS, CSV, Parquet, ROOT, raw arrays) into sealed fd5 files via the existing `fd5.create()` builder API. Also supports importing external metadata (RO-Crate, DataCite) to enrich fd5 files during creation.

See [RFC-001 § Out of scope](docs/rfcs/RFC-001-2026-02-25-fd5-core-implementation.md) — "Ingest pipelines (DICOM, MIDAS, etc.)" was explicitly deferred. This epic brings it in-scope.

See [DES-001 § Component topology](docs/designs/DES-001-2026-02-25-fd5-sdk-architecture.md) — ingest pipelines are modelled as "User code" calling `fd5.create()`. This epic formalises that layer.

## Architecture

```
src/fd5/
  ingest/
    __init__.py
    _base.py           # Loader protocol + shared helpers
    dicom.py           # DICOM series → fd5 (pydicom)
    nifti.py           # NIfTI → fd5 (nibabel)
    raw.py             # raw numpy arrays → fd5
    csv.py             # CSV/TSV tabular data → fd5
    parquet.py         # Parquet columnar data → fd5 (pyarrow)
    root.py            # ROOT TTree → fd5 (uproot) — after spike
    midas.py           # MIDAS event data → fd5
    metadata.py        # RO-Crate / DataCite metadata import
```

### Design decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Location | `fd5.ingest` sub-package in same repo | Loaders are tightly coupled to `fd5.imaging` schemas — must evolve in lockstep |
| Dependencies | Optional extras (`fd5[dicom]`, `fd5[nifti]`, `fd5[parquet]`, `fd5[ingest]`) | Heavy deps don't burden core users |
| Import direction | `fd5.ingest` imports from `fd5.create` and `fd5.imaging` — never the reverse | Clean layer boundary |
| Loader interface | `Loader` protocol in `_base.py` | Consistent API across all formats |
| Provenance | Every loader records `provenance/original_files` with source file hashes | Traceability from fd5 file back to raw input |

## Child issues

### Foundation
- [ ] #109 — `fd5.ingest._base` — Loader protocol + shared helpers

### Format loaders (all depend on #109, independent of each other)
- [ ] #110 — `fd5.ingest.dicom` — DICOM series loader (pydicom)
- [ ] #111 — `fd5.ingest.nifti` — NIfTI loader (nibabel)
- [ ] #112 — `fd5.ingest.raw` — raw/numpy array loader
- [ ] #116 — `fd5.ingest.csv` — CSV/TSV tabular data loader
- [ ] #117 — `fd5.ingest.parquet` — Parquet columnar data loader (pyarrow)
- [ ] #118 — [SPIKE] `fd5.ingest.root` — ROOT TTree loader feasibility
- [ ] #114 — `fd5.ingest.midas` — MIDAS event data loader

### Metadata import
- [ ] #119 — `fd5.ingest.metadata` — RO-Crate and DataCite metadata import

### CLI
- [ ] #113 — `fd5 ingest` CLI commands

## Dependency graph

```
#109 (_base)  ←  #110, #111, #112, #113, #114, #116, #117, #118, #119
#113 (cli)    ←  all loaders (discovers available loaders)
```

`_base` (#109) must be implemented first. All loaders and metadata import are independent of each other and can run in parallel. CLI (#113) depends on at least one loader existing. ROOT (#118) is a spike — implementation issue created only if feasible.

## Success criteria

- Each loader produces a valid fd5 file that passes `fd5 validate`
- Provenance chain traces back to original source files
- ≥ 90% test coverage per loader module
- `pip install fd5` does not pull in pydicom/nibabel/pyarrow; `pip install fd5[dicom]` does
---

# [Comment #1]() by [gerchowl]()

_Posted on September 28, 2026 at 05:53 PM_

Closing as **obsolete** (2026-09-28 backlog triage).

Evidence: Superseded by Tessera's ingest architecture (ADR-0025 + ADR-0056 + ADR-0035). tessera-ingest crate present at tessera/crates/tessera-ingest/.

https://claude.ai/code/session_01XdERKMVDAwfMJSKdTytNnK

