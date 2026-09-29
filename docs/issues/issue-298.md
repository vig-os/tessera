---
type: issue
state: open
created: 2026-07-02T14:14:22Z
updated: 2026-09-28T17:59:59Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/298
comments: 0
labels: none
assignees: none
milestone: backlog / research
projects: none
parent: none
children: none
synced: 2026-09-29T07:52:53.867Z
---

# [Issue 298]: [Spike: Ballista (+Vortex) distributed query — Arrow-native scale-out for cross-object shuffle](https://github.com/vig-os/tessera/issues/298)

Evaluate **Apache DataFusion Ballista** as the Arrow-native distributed rung of the query ladder (EPIC #295, T4). Attraction: stay Rust/Arrow/DataFusion end-to-end — **distribute the *same* logical plans** the embedded engine already runs, Arrow Flight shuffle, **no JVM/Spark**. Ballista runs on the **same K8s** as the fan-out, so it's a *usage-case* choice (needs distributed shuffle/join), not an infra choice.

**Status (2026):** v53.0.0 (May 2026), releasing per-DataFusion; fault-tolerant shuffle + Arrow Flight + monitoring TUI; "working towards production-ready" (DataFusion↔Ballista compat gap remains). **Reference: Spice.ai runs Ballista + Vortex in production** — literally tessera's pairing — study it first.

**Discipline:** most tessera scale is embarrassingly-parallel **fan-out (map)**, not distributed shuffle — reserve Ballista for genuine cross-object joins/global group-bys (rare; else DuckDB/lakehouse). Alternatives to weigh: DataFusion-Ray, Sail (Spark-compatible, JVM-free). Deliverable: spike Ballista+Vortex on K8s, decide gate criteria. Refs: #286
