---
type: issue
state: closed
created: 2026-11-15T12:54:19Z
updated: 2026-11-16T06:56:52Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/869
comments: 2
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-16T08:59:39.744Z
---

# [Issue 869]: [Record the table codec profile beside the sealed Vortex edition?](https://github.com/vig-os/tessera/issues/869)

Follow-up from #635 / PR #788 (review finding).

PR #788 seals `TableSpec.edition` (the Vortex edition, `core2026.08.3`) on every table block. The edition is a
**necessary** condition for "same input → same table bytes", not a sufficient one: it bounds which encoding wire IDs
the writer may use, not which it picks. The choice also depends on

- tessera's codec set: the compressor excludes ALP, ALPRD, integer Pco, Delta and OnPair, all of which are inside
  `core2026.08.3` (ADR-0024 §Codec set), and
- the pinned Vortex's BtrBlocks compressor heuristics.

Both are pinned by the writer build and change only as a format event (ADR-0024), so today a product can't say which
codec set wrote it; only the writer build can.

**Question:** should a sealed table spec also record a codec-profile id (for example `tessera-table-v1`, bumped with
every codec-set change), so that "same input + same edition + same profile → same bytes" can be checked from the
product alone? Trade-offs: one more sealed field and one more manifest_hash move, against a self-describing
determinism contract. A design decision for the owner; nothing is implemented.

Refs: #635

---

# [Comment #1]() by [gerchowl]()

_Posted on November 15, 2026 at 08:33 PM_

Owner decisions 2026-11-15: alpha.6 is cut after #573 and #734 land, as one security release; one GitHub security advisory covers ≤ alpha.5 (the zip reader differential plus the manifest-seal bypasses), drafted for owner review; #869: record the codec-profile id beside the edition.

---

# [Comment #2]() by [gerchowl]()

_Posted on November 16, 2026 at 12:23 AM_

**Owner decision (2026-11-16), from the kit storage discussion (nerdmachines/kit#97):** a tunable table profile means **declared constants**, not adaptive choices.

- A profile is a named, fixed set of constants. The same input with the same edition and the same profile gives the same bytes, for every profile.
- Byte reproducibility stays the requirement for large data files, where encoding is done once. A second profile for collections of small, frequently changed rows may use other constants (smaller leaves, for example), and is still reproducible within itself.
- Nothing is chosen by the writer at run time from the workload. Adaptive layouts and build-dependent encodings stay out, for the reason ADR-0024 gives for ALP.

**What this implies for the question in this issue** (inference, to confirm): once more than one profile exists, a product has to say which one wrote it, so the sealed table spec needs the profile id. That is the "yes" branch above.

**Not decided here:**
- which constants a profile covers. Candidates from the kit side: `BLOCK_ROWS`, `ROWS_PER_GROUP`, the codec set, the sort key, the index leaf size (ADR-0028's measured knee near 2¹⁴ rows), and a delta chain limit;
- the profile names and the values of a small-row profile;
- that `ROWS_PER_GROUP` is frozen for format compatibility today, so letting it vary per profile is a format event (corpus regeneration and cross-architecture byte evidence), to be sequenced with the Vortex jump in #635.

The small-row profile is only worth defining if key lookup and small updates meet their budgets on Vortex leaves plus delta leaves. That measurement is the central experiment in nerdmachines/kit#97.


