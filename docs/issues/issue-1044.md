---
type: issue
state: closed
created: 2026-11-27T20:53:29Z
updated: 2026-11-27T22:06:29Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/1044
comments: 1
labels: chore
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-28T09:46:57.830Z
---

# [Issue 1044]: [chore(release): cut v0.1.0-alpha.8 (ADR-0067 repository layout 2, collections, pins)](https://github.com/vig-os/tessera/issues/1044)

Cut v0.1.0-alpha.8. It is the first release with ADR-0067 complete (#947 closed): repository layout 2 with typed refs, collections as repository objects, gc through members, `commit_collection`/`log`/`history`, pins, and the CLI's exit-code contract (4 retryable, 5 refused, 6 damaged). It is breaking for repositories: an older layout is refused and there is no migration verb. It also includes the leaf-contract refusals (#978, #1011), the typed `SchemaMismatch` (#1021), the thread budget (#703), the HDF5 lock fix (#1041) and the init crash-retry fix (#1007).

The cut follows docs/RELEASING.md:
1. Release PR #993: version the CHANGELOG, then review and merge.
2. Signed tag `v0.1.0-alpha.8` on the merge commit.
3. The Release run.
4. Promotion PR to `main`, approved by the owner.
5. Announce.
---

# [Comment #1]() by [gerchowl]()

_Posted on November 27, 2026 at 10:06 PM_

Released v0.1.0-alpha.8.
- Release PR #993 was merged as e2db1c06, and the signed tag `v0.1.0-alpha.8` (2443a4b1) points at it.
- Release run 46072383784 succeeded on all four platforms and published a prerelease with 13 assets.
- The release notes are only the `[0.1.0-alpha.8]` section, opening with Format break.
- Promotion PR #1045 has the tag's exact tree (fb135cdc) and parents `main` ee277ad3 and the tag commit. It waits for the owner's approval and supersedes #985, #927 and #855.

