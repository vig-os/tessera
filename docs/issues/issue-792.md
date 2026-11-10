---
type: issue
state: open
created: 2026-11-09T15:33:27Z
updated: 2026-11-09T15:33:27Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/792
comments: 0
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-10T09:13:24.430Z
---

# [Issue 792]: [build(deps): unify the two arrow majors (58 pinned, 59 via vortex 0.93)](https://github.com/vig-os/tessera/issues/792)

## Problem

Since the vortex 0.93 bump (#635, PR #788), the graph links **two arrow majors**:

- **arrow 58.4.1**: tessera's own `=`-pins (`arrow-array`, `arrow-buffer`, `arrow-schema`, `arrow-ipc`, plus `parquet` in lockstep), used by the generic-ingest table lane (ADR-0056 / ADR-0057).
- **arrow 59.3.2**: pulled in by vortex 0.93 (`^59.2`) for its own Arrow interop.

It links today because tessera never passes an arrow value to or from vortex. The costs are real, though:

- Two copies get compiled into every binary.
- `scripts/feature-snapshots.sh` now has to snapshot both locked versions of each arrow crate (Gate B).
- Any future tessera ↔ vortex arrow interop would hit a type mismatch. That includes the DataFusion adapter in ADR-0062, which requires DataFusion's arrow major to equal vortex's.

## Constraints

- The ingest pins are `=` on purpose. A decoder bump is a deliberate corpus event (ADR-0056 §5/§6a), because the version goes into the sealed `ingest_decoder` digest. Moving 58 → 59 moves the `manifest_hash` of every ingest-corpus fixture.
- The floor is 58.4.1 for RUSTSEC-2026-0435 (thrift 0.23, #736). arrow and parquet release in lockstep.
- Vortex upgrades move vortex's arrow major on its own schedule.

## To plan

1. Decide the policy: do tessera's arrow pins **follow vortex's major** (unify at every vortex bump, two corpus events per bump at most), or stay independent?
2. If unifying: bump the pins to `=59.x` together with `parquet`. Regenerate `corpus/ingest-corpus.json` as its own reported corpus event, show that only the `ingest_decoder` triple and the `manifest_hash`es move, and check that Gate A still holds across its four feature configurations.
3. Drop the multi-version handling from Gate B once one version remains, or keep it as a guard.
4. Add a check that fails when two arrow majors are locked, if the policy is "unified".

Out of scope for #788, which keeps 58 so the vortex format event stays separate from a decoder event.

