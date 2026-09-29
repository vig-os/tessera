---
type: issue
state: closed
created: 2026-02-25T22:28:58Z
updated: 2026-09-28T17:54:00Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/136
comments: 1
labels: feature, effort:large, area:core
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-09-29T07:53:08.200Z
---

# [Issue 136]: [[FEATURE] Implement fd5.open() read API for consuming fd5 files](https://github.com/vig-os/tessera/issues/136)

### Description

fd5 currently has a `fd5.create()` builder API for writing files, but no corresponding read API. Users who receive or produce fd5 files have no SDK-supported way to open them, access datasets as numpy arrays, browse metadata, check units, or traverse the provenance DAG.

### Problem Statement

The write path (`fd5.create()`) runs once in an ingest pipeline. The read path runs hundreds of times by many users: loading volumes into analysis scripts, checking metadata, extracting MIPs for dashboards, verifying provenance. Without a dedicated read API, users fall back to raw h5py, losing the typed, discoverable experience that fd5's schema makes possible.

### Proposed Solution

Add an `fd5.open()` function (or `fd5.File` class) that returns a typed, read-only wrapper around an HDF5 file. Consider:

- `fd5.open(path) -> Fd5File` — opens and validates the file, exposes product type
- `file.metadata` — returns nested dict (via existing `h5_to_dict`)
- `file.volume` / `file.events` / `file.data` — product-type-appropriate dataset access returning numpy arrays
- `file.units(dataset_name)` — returns unit info for a dataset
- `file.provenance` — returns the provenance DAG as a navigable structure
- `file.sources` — lists source products with IDs and content hashes
- `file.validate()` — runs schema validation on the open file
- Context manager support for resource cleanup

### Alternatives Considered

- **Raw h5py only**: works but loses all schema-awareness, units helpers, and provenance navigation. Users must know the fd5 layout.
- **Xarray backend**: could provide read access via `xarray.open_dataset(path, engine="fd5")`. Complementary to a native API but not a substitute for metadata/provenance access.

### Additional Context

The existing `h5_to_dict` and `dict_to_h5` helpers in `h5io.py`, the units module, and the schema validation module provide building blocks. The read API would compose these into a user-facing interface.

### Impact

- All fd5 users benefit — this is the primary daily interface to fd5 data
- Backward compatible (additive)
- Enables downstream integrations (Jupyter, xarray, visualization tools)

### Changelog Category

Added
---

# [Comment #1]() by [gerchowl]()

_Posted on September 28, 2026 at 05:53 PM_

Closing as **obsolete** (2026-09-28 backlog triage).

Evidence: Tessera ergonomic Python read API landed . See commit dc0d6f5.

https://claude.ai/code/session_01XdERKMVDAwfMJSKdTytNnK

