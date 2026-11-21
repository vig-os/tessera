---
type: issue
state: open
created: 2026-11-21T02:14:04Z
updated: 2026-11-21T02:14:04Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/945
comments: 0
labels: feature, epic, area:core, area:io
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-21T09:10:52.010Z
---

# [Issue 945]: [epic: what the kit's data engine needs from tessera — index of the requests and what blocks kit first](https://github.com/vig-os/tessera/issues/945)

Filed by an agent for the owner, 2026-11-21. This is an index. It adds no requirement of its own: every item below is an existing issue, and each issue keeps its own acceptance.

## What this is

nerdmachines/kit keeps all of its state in tessera: app state, settings, tasks, edges, document blocks, receipts, beside the bulk products tessera already holds. The kit's side of that is now written down as one design in nerdmachines/kit `docs/architecture/data-engine.md` (merged 2026-11-21, nerdmachines/kit#179), reachable from kit's `AGENTS.md`. The open decisions are on nerdmachines/kit#97.

What that design needs from tessera exists here as separate issues. Several were filed from kit's work (#903 to #906, #910, #911, #944); the others were already here. This issue lists them in one place, with what kit uses each for, so that the order can be triaged as one question. Owner direction behind it: storage is tessera's, tessera is ours to shape, and what kit needs is queued here as feature requests (2026-11-18).

## The requests

**Write path for small keyed commits**

- [ ] #905 commit log: one durable record per commit, replay on open, flush into a #781 delta product. Target agreed by the owner on 2026-11-21 (nerdmachines/kit#42): one sync per commit (three today) and no growth over a session.
- [ ] #911 (PR) the commit-cost probe, `bench/ksb/905-commit-cost`. It is the acceptance measurement for #905 and carries the budget below.
- [ ] #907 timer flush, bounded staging latency. #798 group commit.

**Keyed collections: format and read**

- [ ] #780 declared keys, keyed references, a rebuildable key index.
- [ ] #781 delta products over a base, the `current` collection, the newest-row-wins logical read, compaction into a new sealed base.
- [ ] #903 keyed read `get_many(keys, columns)` from encoded sub-chunks, without a scan.
- [ ] #904 small-row table profile as declared constants: sub-chunk and leaf sizes, a key-defined split rule, a delta chain limit (follow-up to #869, closed).
- [ ] #906 retention classes per collection, so that superseded bases and deltas become reclaimable.

**Query**

- [ ] #654 with #666, #667, #668: the ADR-0062 provider over sealed blocks.
- [ ] #944 one provider for a keyed collection: statement snapshot, the visibility rule for deltas, declared semantics for combined graph, vector and relational queries.
- [ ] #768, #769 vector columns and search. #775 full text. #776 graph edge tables and adjacency.

**Open writer: reading what is not sealed yet** (ordered with kit's acquisition work, kit ADRs 0009 and 0010, both at status proposed)

- [ ] #910 read over committed, unsealed fragments at a watermark.
- [ ] #908 ingress tap. #794 commit tap.

**Consuming tessera**

- [ ] #758 a dependent must repeat the Vortex `[patch]`, or it silently builds the crates.io Vortex. kit is such a dependent.

## What kit has measured for these

- **Reads** (nerdmachines/kit#98, vanilla Vortex 0.93.0, 10⁶ rows of about 350 bytes; numbers on #780, #781 and #903): one full row through a scan call about 1.2 ms and megabytes read; from encoded segments held in memory 5.7 µs; a scan through uncompacted deltas linear in their number; after an insert or delete a row-count split shares 0 % of the later leaves, a key-defined split 87.5 to 100 %.
- **Small commits** (three runs of the #905 probe on this repository's `dev` at `81914f6`, comments on #905): the existing path at one row per commit took 3.7, 8.6 and 6.3 ms p50, with three syncs per commit, about 20 times the raw row bytes staged, and a cost that grows with the blocks in the session; one append plus one sync took 0.56, 0.87 and 0.78 ms. All three runs were on a virtiofs mount. The owner ruled on 2026-11-21 that acceptance is judged on reference hardware with local storage; that run is pending.
- **Budget** (owner, 2026-11-19, for the `committed` durability class): for commits of up to 100 rows, p50 at most 2 ms and p99 at most 10 ms; at least 500 commits per second from one writer; cost flat up to 100,000 commits in a session; at most twice the raw row bytes on disk before sealing.

## Order

The order of work is the owner's and the architect's; the triage on #778 (2026-11-09) is the standing one and puts the provider PRs first, then #780, then #768 → #769, #775, #781 and others, with #776 last. It predates #903 to #911 and #944, so those have no place in it yet. What kit can say is which of them block it first:

1. **#905 with #911.** Without it there are no agent-rate durable writes: today's path gives 111 to 256 one-row commits per second in the three runs, against a budget of 500.
2. **#780, then #781 and #903, with #904.** They make a keyed collection readable and changeable by key at all.
3. **#906.** Without a retention class every superseded generation stays reachable.
4. **#666 to #668, then #944.** kit can serve reads from its own in-memory view until the provider exists.
5. **Vectors, full text and graph** as #778 orders them.

#910, #908 and #794 are ordered by the acquisition work, not by this list.

## What kit does not ask for

- A second versioning story. tessera keeps one (#781 with ADR-0036; owner ruling of 2026-11-09). Branches and merges are open on kit's side (nerdmachines/kit#97) and are not requested here.
- Adaptive layouts. A profile is declared constants (owner, 2026-11-16, on #869).
- A row store or a second on-disk format beside tessera's.
- A place for logs about the system. Those are vigil's.

## Related

nerdmachines/kit#97 (decisions), nerdmachines/kit#42 (owner answers of 2026-11-18 and 2026-11-21), nerdmachines/kit#74 (kit-query), nerdmachines/kit#98 (stage-one benchmark), nerdmachines/kit#113 (rerun of the withdrawn write-side results). #778 (order of work), #654 (provider epic).

