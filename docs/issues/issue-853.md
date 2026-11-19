---
type: issue
state: open
created: 2026-11-14T10:18:36Z
updated: 2026-11-18T14:52:21Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/853
comments: 0
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-19T09:52:15.104Z
---

# [Issue 853]: [feat: seeded predicate sampling (sample(n, predicate, seed)) with provenance, on top of Vortex pushdown + take](https://github.com/vig-os/tessera/issues/853)

## Request (from [private-repo]#237, an event browser; consumer [private-repo]#426)
Interactive filtered sampling, such as "100 random coincidences where the energy and dt are in these ranges and the crystal pair is in this set", on ~30 GB event tables.

**Already provided** (tables-sql.md): Vortex filter pushdown with per-chunk statistics, O(1) random `take`, column projection, and DataFusion `sql` over table blocks. The original version of this issue asked for zone maps; those are already covered.

**Remaining ask:**
- **A seeded predicate sample as a first-class read:** `sample(n, predicate, seed)` → rows, plus a `tessera sample` CLI verb. It should be uniform over the matching rows and use pushdown and `take` so it never scans everything.
- **Provenance:** the predicate, seed, method and source seal are recorded in any derived product made from the sample.
- **Doc note:** how fast "first N matching rows" and "uniform N of the matches" are relative to a full scan.

This is scanner-agnostic: visionx-rs is the first consumer.

