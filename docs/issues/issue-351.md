---
type: issue
state: closed
created: 2026-08-02T13:09:13Z
updated: 2026-09-29T02:32:18Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/351
comments: 0
labels: none
assignees: none
milestone: 0.1.0-alpha.2
projects: none
parent: none
children: none
synced: 2026-09-29T07:52:45.846Z
---

# [Issue 351]: [docs(table): reads don't surface the parallel runtime or the intended access pattern](https://github.com/vig-os/tessera/issues/351)

## Docs: reads don't surface the parallel runtime or the intended access pattern

**Summary.** A good-faith integrator can read the entire `tessera-io::table` API
surface, wire a working read, and still end up on the *slowest possible* path —
because the two things that decide read performance are undiscoverable from the
signatures:

1. **Which runtime drives the scan.** `runtime_session()` (the helper every
   `decode*` copies) builds a bare single-threaded `CurrentThreadRuntime`. Vortex
   ships a multi-core `CurrentThreadWorkerPool` (`rt.new_pool()` +
   `set_workers_to_available_parallelism()`), but nothing in the table docs points
   to it. An integrator who reuses `runtime_session` — the obvious move — gets
   single-core reads.
2. **The intended consumption shape.** The module doc advertises "zero-copy
   Arrow→DuckDB / filter-pushdown / O(1) random-take", but the only exposed table
   read (`decode`) *full-materialises every value into `ColumnData`*. Benching that
   path against a mature row store (parquet) shows Vortex ~4× slower — which reads
   as "Vortex is slow" rather than "you benched the one access pattern a columnar
   store is worst at."

**What actually happened (the motivating case).** I copied `runtime_session`'s
single-thread runtime into a hand-rolled scan loop, benched full-materialise-to-
`Vec<struct>`, and wrote down "the read gap is Vortex decode throughput." All
three wrong: pairing the runtime with the worker pool recovered ~15–20% (the
single-thread runtime *was* the first bottleneck), and the residual is the
full-materialise access pattern, not the format.

**Proposed doc changes** (happy to PR — I have a draft):
- A "Reading — performance & the intended access pattern" section on the
  `table` module that (a) tells readers to drive `decode*` on the worker pool
  (or documents that the library now does, if that lands), and (b) states plainly
  that full-materialise-to-host-structs is the slow path and projection +
  zero-copy-Arrow is the fast one.
- A one-line warning on `runtime_session` (or wherever the runtime is chosen)
  that it is single-threaded and not for throughput reads.
- If there's an intended zero-copy/arrow table-read entry point, surface it in the
  module docs next to `decode` (right now `decode` looks like *the* read).

Willing to open the PR (I've prototyped the pooled `READ_RT` + a
`decode_projected` single-session projection locally). Filing the issue first per
the discoverability angle.

