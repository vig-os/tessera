---
type: issue
state: open
created: 2026-10-29T02:48:42Z
updated: 2026-10-29T02:48:42Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/690
comments: 0
labels: area:io
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-29T08:04:01.216Z
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

