---
type: issue
state: closed
created: 2026-11-19T16:59:15Z
updated: 2026-11-19T18:05:03Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/925
comments: 0
labels: bug, priority:high, area:docs
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-20T09:39:08.613Z
---

# [Issue 925]: [bug(ci): promote-main cannot open the promotion PR — the merges-API merge commit is unsigned and the Signed commits ruleset refuses it](https://github.com/vig-os/tessera/issues/925)

## Problem
`promote-main.yml` cannot open the promotion PR for v0.1.0-alpha.6. Run https://github.com/vig-os/tessera/actions/runs/44571980870 failed with:

```text
gh: Repository rule violations found
Commits must have verified signatures.
(HTTP 409)
GitHub refused to merge v0.1.0-alpha.6 into main's tip (ee277ad3…): a conflict, or a permission the App lacks
```

## Root cause
- main is `ee277ad3` (alpha.1), which is not an ancestor of the `v0.1.0-alpha.6` tag (`7ac85bd5`). So `scripts/promotion-plan.sh` picks `HEAD_KIND=merge` and asks GitHub's merges API (`POST /repos/{o}/{r}/merges`) to merge the tag into a branch cut at main.
- The **Signed commits** ruleset (id 24312957, `required_signatures` on `~ALL` refs, no bypass actors) refuses that merge commit, because the merges API does not produce a verified signature for the App. This is not a conflict.
- The `merge` path has never succeeded. The alpha.5 promotion PR #855 has the bare tag commit as its head instead, and is `BLOCKED` behind an update-branch, the case the workflow's header says it exists to avoid.

## Proposed fix (root cause, no ruleset weakening)
Build the promotion head with the Git Data API instead of the merges API:
- `POST /repos/{o}/{r}/git/commits` with `tree` = the tag's tree, `parents` = [main_sha, tag_sha], and **no** `author`/`committer`. GitHub signs commits that an App creates through this endpoint without custom identities, so the commit shows as verified.
- Then create `refs/heads/release/promote-<tag>` at it.
- The head's tree is the tag's tree by construction, so the "tree must equal the tag tree" invariant holds with no merge at all, and there is no conflict case left to handle.
- Verify `.verification.verified == true` on the created commit before opening the PR. Fail loudly otherwise, with no fallback.

Rejected: adding the App as a bypass actor on Signed commits. That weakens a rule that covers every ref.

## Acceptance
- `scripts/promotion-plan.sh` self-tests cover the new path.
- A manual `gh workflow run promote-main.yml -f tag=v0.1.0-alpha.6` opens a PR whose head commit is verified, whose tree equals the tag's tree, and whose parents are [main, tag].
- `nix flake check` on the PR is green.
- #855 (alpha.5) is superseded. The owner closes it once the alpha.6 PR exists.

Refs: #854

