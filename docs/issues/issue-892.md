---
type: issue
state: open
created: 2026-11-16T14:11:51Z
updated: 2026-11-23T16:44:39Z
author: vigos-devkit-upgrade[bot]
author_url: https://github.com/vigos-devkit-upgrade[bot]
url: https://github.com/vig-os/tessera/issues/892
comments: 1
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-24T11:59:41.139Z
---

# [Issue 892]: [chore(devkit): automated devkit upgrade is failing](https://github.com/vig-os/tessera/issues/892)

<!-- devkit-upgrade-failure -->

The managed devkit-upgrade workflow is failing, so this repo has stopped adopting new devkit releases.

- Run: https://github.com/vig-os/tessera/actions/runs/43882845082
- Failing step: Publish the adoption commit via API (verified)
- Pinned devkit version: 1.17.0
- Target devkit version: 1.19.0
Nothing was pushed: the adoption branch only reaches the remote once the upgrade commit is published, so there is no half-applied state to clean up. Fix the cause and re-run the workflow (or dispatch it with an explicit version) — this issue is filed, updated and closed by the workflow itself.
---

# [Comment #1]() by [vigos-devkit-upgrade[bot]]()

_Posted on November 23, 2026 at 04:44 PM_

The devkit upgrade failed again.

- Run: https://github.com/vig-os/tessera/actions/runs/45219581025
- Failing step: Publish the adoption commit via API (verified)
- Pinned devkit version: 1.17.0
- Target devkit version: 1.19.2

