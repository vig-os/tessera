---
type: issue
state: open
created: 2026-11-15T09:57:46Z
updated: 2026-11-15T09:57:46Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/864
comments: 0
labels: area:ci
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-16T08:59:41.367Z
---

# [Issue 864]: [ci(promote-main): test the branch-cleanup rule and fix a stale comment (nits from #856)](https://github.com/vig-os/tessera/issues/864)

From the fresh re-review of #856 at 4b83a3f. Not blocking.
- The refused-merge cleanup (delete the promotion branch only while it still points at `MAIN_SHA`, never a pre-existing branch) is inline workflow shell (`promote-main.yml:152-183`), so the `release-promotion` check covers only the planner. Move the decision into `scripts/` with a unit test covering: a pre-existing branch, a branch created by this run, a failed GET (empty) and a branch moved by another actor.
- `promote-main.yml:73` still says 'mints the release App token'. It now uses the vigos-devkit-upgrade App.
- The first live run happens on the next release. Verify it opens the promotion PR, and record the result here.

Refs: #854
