---
type: issue
state: open
created: 2026-11-19T11:49:01Z
updated: 2026-11-19T11:49:01Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/921
comments: 0
labels: feature, priority:low, area:io
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-20T09:39:10.498Z
---

# [Issue 921]: [perf(io): page-granular Pco take/filter kernel (upstream vortex-pco) — reader-side follow-up to tessera-table-v2 (O2)](https://github.com/vig-os/tessera/issues/921)

Follow-up from the accepted tessera-table-v2 design note (#915, PR #919). It records owner decision O2 (2026-11-19).

Under v2, a random `take` over a Pco column decodes the whole 65,536-row group, because the current Vortex Pco array canonicalises the whole chunk (`vortex-pco` `filter/vtable.rs:166-184`). The owner accepted that cost, with the note's A4 latency floors. This issue tracks removing the cost on the reader side.

**Scope.** Add page-granular `take` and `filter` kernels to `vortex-pco`, upstream in Vortex. They decode only the 8,192-value pages a requested row hits. Pages can already be decoded independently (`array.rs:641-730`). The stored bytes and the profile id do not change, so this is a reader-only change and no hash moves.

**Process.**
- The change goes upstream to Vortex first. It needs the owner's go and an AI disclosure.
- Then the fork pin (#480) and tessera pick it up.

**Acceptance.**
- Take latency for 1, 500 and 5,000 uniform random rows, and for a 500-row sorted cluster, on a full 2^22-row block (measurement M2 in the note), beats A4's floors.
- Results are bit-identical to whole-chunk decode.
- No golden moves.

Depends on #828, which makes v2 the default.

