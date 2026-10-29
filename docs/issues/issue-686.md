---
type: issue
state: closed
created: 2026-10-26T19:45:16Z
updated: 2026-10-29T06:06:39Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/686
comments: 2
labels: bug, area:io
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-29T08:04:02.329Z
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

---

# [Comment #2]() by [gerchowl]()

_Posted on October 29, 2026 at 04:52 AM_

Fold into #686 (from the PR #691 review at ff9d76f; `recover` is the same code #686 rewrites):

1. `tessera-io/src/write.rs:709`/`:712`: the docs say a fragment named by a journal line *after* the recovery stop point "is kept … so a journal cut short can still be repaired by hand". But `recover` rewrites the journal to the clean prefix before it removes orphans, so a second `recover` deletes those kept fragments. A kept `c/0` directory also makes a later append of `c` fail with EISDIR instead of `BlockNameConflict`. Make the documented behaviour true, either by keeping the journal lines that name those fragments or with an explicit quarantine. Pin it with a test that runs recover twice.
2. `write.rs:686`/`:689`: a complete JSON line that is not a valid `BlockRef` is treated as a torn tail (`break`) instead of being refused. That contradicts the rule that a complete line is a commit. Refuse it with a typed error, and test it.

