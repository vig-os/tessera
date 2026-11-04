---
type: issue
state: open
created: 2026-11-02T08:49:53Z
updated: 2026-11-04T05:01:16Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/730
comments: 2
labels: priority:low, area:ci
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-04T08:35:56.733Z
---

# [Issue 730]: [ci(nix-check): a dispatch run on a tag ref is filed as a release-tag failure; watch the first real nightly](https://github.com/vig-os/tessera/issues/730)

Follow-up nits from the review of PR #727 (#725), head 6ab2f87.

1. `.github/workflows/nix-check.yml:302`: a `workflow_dispatch` run on a tag ref also matches `startsWith(github.ref,'refs/tags/')`. It would be filed under "run on release tag …", which contradicts the header's "dispatched runs are not filed". Fix: also require `github.event_name == 'push'`, with a self-test case.
2. The scheduled path (the dev ref check plus the `report` job writing a real issue with the real token) has not yet run end to end. Only the stubbed self-test and DRY_RUN have exercised it. Check the first nightly after the merge; if it is green, force a failing scheduled-equivalent path once, or document why the self-test is sufficient.

Refs: #725
---

# [Comment #1]() by [gerchowl]()

_Posted on November 3, 2026 at 06:25 AM_

Watch item update (2026-11-03): the first nightly did NOT run. `cron: '17 2 * * *'` has been on dev (the default branch) since PR #727 merged at 2026-11-02T08:49Z, but `GET actions/workflows/365879045/runs?event=schedule` returns total_count 0, so no scheduled run was created at 2026-11-03T02:17Z (not queued, not failed: absent). Only the two manual workflow_dispatch runs from 2026-11-02 exist. Possible causes still to tell apart: GitHub dropping a schedule under load (a documented possibility), or the workflow's trigger/permissions keeping the schedule from registering. If 2026-11-04T02:17Z also produces no run, it is not a one-off drop; diagnose the trigger, because the aarch64 determinism gate currently has NO automatic run at all.

---

# [Comment #2]() by [gerchowl]()

_Posted on November 4, 2026 at 05:01 AM_

Item 2, status: the first nightly ran green on both legs. Run [41611702325](https://github.com/vig-os/tessera/actions/runs/41611702325) is a schedule run on dev (the workflow's display name is `CI`). GitHub created it at 2026-11-03T09:31Z, about 7 h after the 02:17 cron.
- plan: success
- aarch64-linux: success (09:31→10:19)
- x86_64-linux: success (09:57→11:08)
- the gate: success
- `file the failure`: skipped, as it should be on a green run.

So the dev-ref check and both legs are proven on a real schedule event. The report job's real-token filing path is still unexercised; that part of item 2 stays open, and so does item 1.

