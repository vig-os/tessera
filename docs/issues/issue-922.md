---
type: issue
state: open
created: 2026-11-19T11:51:21Z
updated: 2026-11-19T11:51:21Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/922
comments: 0
labels: docs, priority:low, area:docs
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-20T09:39:10.018Z
---

# [Issue 922]: [docs(design): tessera-table-v2 note wording nits (review of #919)](https://github.com/vig-os/tessera/issues/922)

Review nits on the accepted tessera-table-v2 note, `docs/spikes/915-tessera-table-v2.md` (merged in #919 at 12444ed):

1. Line 6 says the note "proposes" the ADR-0024 amendment. The note is Accepted, so this should read "decides". The amendment itself lands with #828.
2. Lines 125 and 350 quote the error as "encoding not found". Vortex's actual error is `Unknown encoding: <id>` (`vortex_bail!`, `serde.rs:375`).
3. M6 includes native 8-bit Pco, the option O1 rejected. A stock Vortex reader cannot decode it (`unreachable!` at `array.rs:298`), so it can only be measured with the `pco` crate on its own. Say so.
4. Line 266 is 179 characters long; wrap it at 120. Line 89, "Required: O1 is decided (b).", is a fragment; make it a full sentence.

