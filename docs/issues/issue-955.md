---
type: issue
state: closed
created: 2026-11-22T20:45:07Z
updated: 2026-11-22T22:57:11Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/955
comments: 1
labels: bug, area:io, area:integrity
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-23T10:16:27.845Z
---

# [Issue 955]: [fix(io): gc silently deletes the history of a version it can no longer read](https://github.com/vig-os/tessera/issues/955)

Found in the fresh review of PR #942 round 5 (head 2b01959f). The reviewer probed this, so it is more than a code reading.

`Repository::gc` skips any manifest it cannot read (`tessera-io/src/repo.rs:394-397`, `Err(_) => continue`). The skip predates #942, where it caught repeated block names. Since #942, a version holding a dangling chunk index is also refused on read, so this case is new.

**Probe:** import a product, make one `--set` commit, then plant an orphan-index tip on top. `gc` then prints `scanned 5 · kept 1 · reclaimed 4` and exits 0 with no warning. It deleted the older versions and the volume block.

**Expected:** gc never reclaims an object reachable from a ref just because one manifest on the path cannot be read. Either refuse to run (exit ≠ 0, naming the unreadable object and the ref that reaches it) or treat the unreadable manifest as live along with everything it names. Choose one with a failing test first. Relates to #946 (gc can race a commit) and ADR-0036 §2.

Refs: #720
---

# [Comment #1]() by [gerchowl]()

_Posted on November 22, 2026 at 10:57 PM_

Duplicate of #946 item 5 ("`gc` treats 'cannot read' as 'has nothing to keep'"), which the kit storage review filed one day earlier. The probe here (import, a `--set` commit, then a planted orphan-index tip; `gc` printed `scanned 5 · kept 1 · reclaimed 4` and exited 0) is concrete evidence for that item, and #942 (now merged) widened what triggers it. Carried to #946.

