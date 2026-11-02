---
type: issue
state: open
created: 2026-11-02T08:49:53Z
updated: 2026-11-02T08:49:53Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/730
comments: 0
labels: priority:low, area:ci
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-02T08:51:34.404Z
---

# [Issue 730]: [ci(nix-check): a dispatch run on a tag ref is filed as a release-tag failure; watch the first real nightly](https://github.com/vig-os/tessera/issues/730)

Follow-up nits from the review of PR #727 (#725), head 6ab2f87.

1. `.github/workflows/nix-check.yml:302`: a `workflow_dispatch` run on a tag ref also matches `startsWith(github.ref,'refs/tags/')`. It would be filed under "run on release tag …", which contradicts the header's "dispatched runs are not filed". Fix: also require `github.event_name == 'push'`, with a self-test case.
2. The scheduled path (the dev ref check plus the `report` job writing a real issue with the real token) has not yet run end to end. Only the stubbed self-test and DRY_RUN have exercised it. Check the first nightly after the merge; if it is green, force a failing scheduled-equivalent path once, or document why the self-test is sufficient.

Refs: #725
