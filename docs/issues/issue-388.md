---
type: issue
state: open
created: 2026-08-19T09:19:39Z
updated: 2026-09-29T05:11:29Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/388
comments: 1
labels: none
assignees: none
milestone: 0.1.0-alpha.2
projects: none
parent: none
children: none
synced: 2026-09-29T07:52:42.199Z
---

# [Issue 388]: [feat(cli): `tessera bench compare` — head-to-head size + latency vs Parquet/HDF5](https://github.com/vig-os/tessera/issues/388)

**AX audit finding (evaluator, discoverability 2/5).** The docs never make the case vs Parquet/HDF5 with
numbers — the only quantitative claim is −21%/−33% vs *zstd*. `tessera bench` measures only the current
host's write engine, with no Parquet/HDF5 baseline.

## Ask
A `tessera bench compare <in>` (or synthetic) that writes the same table/array to `.tsra`, Parquet, and
HDF5 and reports: on-disk size, cold-cache column-projection latency, ROI-slice latency, (optionally)
range-read-over-S3 bytes fetched. One repeatable command produces the numbers `docs/why-tessera.md`
currently promises as a follow-up. Refs the AX/onboarding audit (#386).
---

# [Comment #1]() by [gerchowl]()

_Posted on September 29, 2026 at 05:11 AM_

Reopened: stage 1 (.tsra vs HDF5) landed in #487. Stage 2 (the Parquet row, using the arrow/parquet crates) waits on #460. Stage 3 (backporting median/cold/seal-verify into bench/ecosystems) is #485.

