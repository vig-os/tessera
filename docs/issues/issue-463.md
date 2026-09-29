---
type: issue
state: closed
created: 2026-09-28T21:13:49Z
updated: 2026-09-28T23:35:08Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/463
comments: 2
labels: none
assignees: none
milestone: 0.1.0-alpha.2
projects: none
parent: none
children: none
synced: 2026-09-29T07:52:29.246Z
---

# [Issue 463]: [ci: stacked PRs get no CI — ci/nix-check/codeql only trigger on PRs into dev, release/**, main](https://github.com/vig-os/tessera/issues/463)

`ci.yml`, `nix-check.yml` and `codeql.yml` trigger on `pull_request: branches: [dev, 'release/**', main]`. A PR whose base is another feature branch (a stacked PR, e.g. #461 on #460) gets **no** lint, test, commit-message or nix checks. Only the cargo-dist `plan` job runs. It stays untested until its base merges and GitHub retargets it.

Current workaround: run the workflows manually with `workflow_dispatch` on the branch.

Options:
- also trigger on `pull_request` for any base (`branches: ['**']`), with the concurrency group from #363 so superseded runs cancel;
- or trigger on `push` to non-default branches;
- or document `workflow_dispatch` as the stacked-PR procedure.

Take the approach the devkit scaffold supports, so drift checks stay green.

Refs: #460, #461
---

# [Comment #1]() by [gerchowl]()

_Posted on September 28, 2026 at 11:31 PM_

Fixed by #473: ci, nix-check and codeql now run on stacked PRs, with per-ref concurrency groups.

---

# [Comment #2]() by [gerchowl]()

_Posted on September 28, 2026 at 11:35 PM_

A sibling failure mode worth recording here, because the **symptom is identical** to this issue and the
cause is different — which cost a while to tell apart on #460.

**A PR that is `mergeable=CONFLICTING` / `mergeStateStatus=DIRTY` gets no CI at all.** GitHub cannot
compute a merge ref for a conflicting PR, so `pull_request`-triggered workflows never fire — not queued,
not cancelled, simply never created. The push lands (GitHub updates `headRefOid` and shows the new commit
on the PR), other PRs keep getting runs normally, and the affected PR just reports
`no checks reported on the '<branch>' branch`.

So there are now **two** distinct reasons a PR sits silent rather than red:

| cause | check |
| --- | --- |
| base branch not in the trigger list (this issue) | `gh pr view N --json baseRefName` |
| PR conflicts with its base | `gh pr view N --json mergeable,mergeStateStatus` |

Both look like a slow queue. Neither resolves by waiting — in the conflicting case, nothing will ever
run until the conflict is cleared, so `git merge origin/dev` (or a rebase, if nothing is stacked on the
branch) is what unblocks CI rather than a re-push or a `workflow_dispatch`.

Suggested first move when a PR reports no checks:

```sh
gh pr view <N> --json baseRefName,mergeable,mergeStateStatus
```

Worth a line in whatever the stacked-PR procedure ends up being, since a stack makes the second cause
*more* likely: every merge into the base branch is another chance for the branch above it to go DIRTY,
and the stack hides it — the upper PR was already not reporting checks for the reason in this issue.


