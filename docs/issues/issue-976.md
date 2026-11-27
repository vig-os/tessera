---
type: issue
state: closed
created: 2026-11-24T20:59:37Z
updated: 2026-11-27T04:28:20Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/976
comments: 1
labels: chore, area:ingest
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-27T10:31:07.789Z
---

# [Issue 976]: [nits from the #972 review (parquet 58.4.2)](https://github.com/vig-os/tessera/issues/976)

1. `tessera/crates/tessera-ingest/src/decoder.rs:31` still shows `"=58.4.1"` in its doc example.
2. The commit message and PR body say "7 parquet fixtures". It is 6 `arrow-rs/parquet` fixtures plus 1 `arrow-rs/arrow-ipc` fixture.

Refs: #970
---

# [Comment #1]() by [gerchowl]()

_Posted on November 27, 2026 at 02:58 AM_

Point 2 is resolved here: the correct count is six arrow-rs/parquet fixtures plus one arrow-rs/arrow-ipc fixture. The stale wording is in the merged commit history and cannot be edited, so there is no remaining code or documentation change for that point. The decoder example is corrected by PR #1018.

