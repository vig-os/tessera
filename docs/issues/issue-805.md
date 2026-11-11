---
type: issue
state: open
created: 2026-11-11T01:15:14Z
updated: 2026-11-11T01:15:14Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/805
comments: 0
labels: feature
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-11T08:50:39.758Z
---

# [Issue 805]: [feature(capi): a read side for C++ consumers (logical tables as Arrow C Data Interface arrays)](https://github.com/vig-os/tessera/issues/805)

## Gap (from the #801 consumer gap analysis, `docs/spikes/801-consumer-gap-analysis.md`)

#800 is a **writer** bridge, but one of the #803 readers is C++. gPET-sim2listmode reads the g4ring output in C++. Its input surface:
- `ListMode-Libraries/DataManagers/src/SimulationDataManager.cpp`: open the shard, open `events`/`pethits`, then `ReadCompound`. The event-id pre-pass reads one column of every shard.
- `ActivityMap.cpp`: a 3-D float64 activity array.

The repo is MorePET/gPET-sim2listmode at `3d6d566`. Once gPET-sim writes `.tsra` through #800, sim2listmode has no tessera reader at all. The only readers are Rust (`Reader::logical_table`, `LogicalTableView::column/take/locate`, `tessera/crates/tessera-io/src/multiblock.rs:39-392`) and Python.

## Ask
A read side in the C ABI. This is an ADR question for #800's ADR: one crate or two, and one header.
- `open`, `logical_table(name)`, `columns`, `row_count`.
- Column projection and row-range reads exported as Arrow C Data Interface arrays (`ArrowArray` + `ArrowSchema`, released by the callee).
- The same error model and ABI versioning as the writer.
- The array-block read for the activity map.

Without it, #803's sim2listmode port can't start.

