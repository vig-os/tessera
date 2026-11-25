---
type: issue
state: open
created: 2026-11-19T11:51:21Z
updated: 2026-11-24T18:10:20Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/922
comments: 1
labels: docs, priority:low, area:docs
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-25T10:52:03.657Z
---

# [Issue 922]: [docs(design): tessera-table-v2 note wording nits (review of #919)](https://github.com/vig-os/tessera/issues/922)

Review nits on the accepted tessera-table-v2 note, `docs/spikes/915-tessera-table-v2.md` (merged in #919 at 12444ed):

1. Line 6 says the note "proposes" the ADR-0024 amendment. The note is Accepted, so this should read "decides". The amendment itself lands with #828.
2. Lines 125 and 350 quote the error as "encoding not found". Vortex's actual error is `Unknown encoding: <id>` (`vortex_bail!`, `serde.rs:375`).
3. M6 includes native 8-bit Pco, the option O1 rejected. A stock Vortex reader cannot decode it (`unreachable!` at `array.rs:298`), so it can only be measured with the `pco` crate on its own. Say so.
4. Line 266 is 179 characters long; wrap it at 120. Line 89, "Required: O1 is decided (b).", is a fragment; make it a full sentence.

---

# [Comment #1]() by [gerchowl]()

_Posted on November 24, 2026 at 06:10 PM_

Two sourcing nits from the re-review of PR #958 (merged), on the note's §10 and `docs/spikes/828-m4b-a3.md`:
1. The report's §6 says "all `pco::` symbols sum to 66.1 %", but the rows it lists add up to only 57.2 %. The other 8.9 % sits in symbols below its cut-off, so print them or cite PR #949's M4.
2. Siemens' "about −0.4 %" rests on M1 per-column bytes (1.126 → 1.097, 0.002 → 0.005) that appear in PR #949's body, not in the report. Copy them into the report.

Refs: #828

