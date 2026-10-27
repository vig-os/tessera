---
type: issue
state: open
created: 2026-10-26T19:45:16Z
updated: 2026-10-26T19:47:04Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/686
comments: 1
labels: bug, area:io
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-27T07:16:17.936Z
---

# [Issue 686]: [fix(io): WriteSession::recover rewrites the journal non-atomically (crash during recovery loses it)](https://github.com/vig-os/tessera/issues/686)

Found during the review of #682 (#677 PR 2); it was already on dev at 41de440.

`WriteSession::recover` (`tessera/crates/tessera-io/src/write.rs:294`) rewrites the cleaned journal with
`fs::write(dir.join(JOURNAL), &clean)`: an in-place truncate and write, with no temp file, no rename and no fsync of
the file or its directory. A crash or power loss during recovery can leave the journal truncated or empty, which is
exactly the situation recovery exists for, and every committed block it named is then lost.

## Fix
Use the crate's existing durable-write path (`write_durable`, used for the header at `:131`): temp file, fsync, rename,
directory fsync. If `write_durable` doesn't give rename atomicity yet, fix it there, not in `recover`.
Test with a fault seam that fails after the truncate and before the write completes: the original journal must
survive, and a second `recover` must give the same blocks.
---

# [Comment #1]() by [gerchowl]()

_Posted on October 26, 2026 at 07:47 PM_

Priority note from the #682 round-2 review: after #682, a session poisoned by a failed journal write can only continue through `recover`, so this non-atomic rewrite now sits on the error path that poisoning relies on.

