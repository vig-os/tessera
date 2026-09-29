---
type: issue
state: closed
created: 2026-07-07T08:37:57Z
updated: 2026-09-28T17:55:12Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/344
comments: 1
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-09-29T07:52:47.056Z
---

# [Issue 344]: [bug(cli): collection ls|verify can't resolve members — on-disk 'blake3_<hash>.tsra' vs reference 'blake3:<hash>'](https://github.com/vig-os/tessera/issues/344)

## Bug

`tessera collection ls` / `collection verify` report **every member as `<member file not found>`**. The `collection.json` references members by `blake3:<hash>` (colon), but the sealed files on disk are named `blake3_<hash>.tsra` (underscore, colon isn't a legal path char). The resolver doesn't bridge the two, so a user cannot cryptographically verify a catalog end-to-end.

Reproduces on **both** `DP01/` (32 members) and `DP01-singles/` (8 members) — a tooling gap, not a data gap. Two independent fresh-context reviewers hit it.

## Fix options

1. Resolver tries `blake3_<full-hash>.tsra` (translate `:`→`_`) when locating a member reference in the collection's directory. (Least friction — makes the existing on-disk layout work.)
2. Also honor descriptive symlinks already present alongside the hash files.

Refs #272 (collection consumer verbs), #304 (assemble collection). Blocks trustworthy `collection verify`.
---

# [Comment #1]() by [gerchowl]()

_Posted on September 28, 2026 at 05:55 PM_

Closing as **duplicate-of #323** (2026-09-28 backlog triage).

Evidence: commit 6077b4d 'fix(cli): collection verify/ls resolve members by the sanitized name (#323) (#339)' — same bug, one SSoT via tessera_core::collection::{sanitize_reference, member_filename}. #323 filed first; #344 files it again.

https://claude.ai/code/session_01XdERKMVDAwfMJSKdTytNnK

