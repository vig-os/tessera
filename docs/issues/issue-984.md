---
type: issue
state: closed
created: 2026-11-25T03:05:51Z
updated: 2026-11-25T05:22:15Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/984
comments: 0
labels: chore, docs
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-25T10:51:44.177Z
---

# [Issue 984]: [docs(release): RELEASING step 1 fails as written (branch-name hook; changelog.py not executable)](https://github.com/vig-os/tessera/issues/984)

Found while cutting v0.1.0-alpha.7 with the runbook from PR #983 (#982):
1. **The local `branch-name` hook refuses the commit** on the release-plz branch `chore/release-plz-<RFC 3339 timestamp>`. CI exempts this PR because a bot opened it, and it keys that exemption on the PR author's account type, not the branch name (ci.yml, the #629 local patch). The local hook mirrors CI's convention but can't see the PR author. Step 1 must say to commit and push with `SKIP=branch-name`, which skips only that one hook, never `--no-verify`, and why. The alternative is to make the local hook recognise the release-plz branch, but the CI comment deliberately avoids keying on names, so the runbook fix is preferred.
2. **`scripts/changelog.py` is not executable** (mode 100644), but the runbook calls it directly: `scripts/changelog.py release …` fails with `Permission denied`. Either set +x (with a shebang, if it lacks one) or write `python3 scripts/changelog.py` in the docs and in the flake check.

Refs: #917
