---
type: issue
state: open
created: 2026-10-29T02:48:42Z
updated: 2026-11-15T14:30:37Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/690
comments: 1
labels: area:io
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-16T09:00:43.781Z
---

# [Issue 690]: [fix(core,io): refuse Windows device names and case-fold collisions in block-name paths](https://github.com/vig-os/tessera/issues/690)

Follow-up to #685 / ADR-0063 (block-name grammar).

ADR-0063 makes every block name a `/`-separated path of `[A-Za-z0-9._-]+` segments, and maps a name onto
a directory only through `BlockName::path_under`. Two filesystem hazards remain that the grammar does
not cover. Neither is reachable on the platforms Tessera ships today (Linux, macOS; Windows-msvc is
deferred in `dist-workspace.toml`), but both must be closed before a Windows build ships, or before
exploded products are routinely moved to case-insensitive filesystems.

1. **Windows reserved device names.** `CON`, `PRN`, `AUX`, `NUL`, `COM1`–`COM9`, `LPT1`–`LPT9` (with or
   without an extension, e.g. `NUL.txt`) are grammar-valid. On Windows, writing one as a file opens the
   device. Proposed: `path_under` (and `join_checked`) refuse a segment whose stem matches one, on all
   platforms or only when building a Windows path. The owner decides which, since refusing everywhere
   changes the grammar.
2. **Case-insensitive filesystems.** `Volume` and `volume` are distinct names, but on macOS (default
   APFS) and Windows they are one file, so `unpack` of a product holding both would overwrite one with
   the other. Proposed: `unpack`/`WriteSession` refuse a case-fold collision (typed error), like the
   leaf rule.

Acceptance: a test per hazard; ADR-0063's "Limitations" section updated.

---

# [Comment #1]() by [gerchowl]()

_Posted on November 15, 2026 at 02:30 PM_

Update from PR #862 (round 2 of the GHSA-m665-v7pm-7grj fix). Full verification now refuses an archive whose
`aux/` names collide up to ASCII letter case or break the leaf rule (`aux/a` next to `aux/a/b`), and the aux
writers (`add_aux_members` / `write_aux_members`, `pack_dir`) refuse to write such a set. That covers the
unsealed half. Three hazards stay open here, all still grammar-valid:

1. **Case-fold collisions among sealed `blocks/` names** (`Volume` next to `volume`). The verifier leaves these
   alone: the grammar admits them and old products may hold them, so refusing them is a format decision for this
   issue (seal-time rule, reader rule, or both), not something to add silently to `verify`.
2. **Windows device names**, as in item 1 above, for `aux/` names as well as block names (`aux/CON` verifies
   today).
3. **A trailing `.` or space on a segment.** Windows drops it, so `aux/y.` and `aux/y` (or `blocks/v.` and
   `blocks/v`) are one file there. The grammar permits a trailing `.`.

An adversarial pass on Linux found all four readers (unzip, Python `zipfile`, `bsdtar` streaming, 7z) extract
identical trees for `aux/CON`, `aux/y` + `aux/y.` and dotfiles. So these diverge only on Windows or on
case-insensitive file systems, consistent with this issue's scope.


