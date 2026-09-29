---
type: issue
state: closed
created: 2026-07-01T14:25:15Z
updated: 2026-09-28T17:54:43Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/272
comments: 1
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-09-29T07:52:58.901Z
---

# [Issue 272]: [feat(cli): collection consumer verbs — tessera collection inspect/ls/verify](https://github.com/vig-os/tessera/issues/272)

Found by the ingest reviewer. `ingest --spec` emits `collection.json` but there's no verb to read it, and `tessera inspect collection.json` fails with raw `container: invalid Zip archive: Could not find EOCD`. Add `tessera collection inspect/ls/verify <collection.json>` (members, roles, sizes, MMR-root verify — ties to #223) + human-friendly per-member filenames (`--name-members-by name`) alongside the `blake3_…` content-addressed name.
---

# [Comment #1]() by [gerchowl]()

_Posted on September 28, 2026 at 05:54 PM_

Closing as **done** — verified on `origin/dev` in the 2026-09-28 backlog triage.

Evidence: commit 257dd87 'feat(cli): collection consumer verbs — inspect / ls / verify (#272) (#282)'.

https://claude.ai/code/session_01XdERKMVDAwfMJSKdTytNnK

