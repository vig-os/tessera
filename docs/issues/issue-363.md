---
type: issue
state: closed
created: 2026-08-04T11:06:49Z
updated: 2026-09-28T21:36:57Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/363
comments: 1
labels: none
assignees: none
milestone: 0.1.0-alpha.2
projects: none
parent: none
children: none
synced: 2026-09-29T07:52:44.751Z
---

# [Issue 363]: [ci: no concurrency group — a force-push leaves the superseded run building for ~45 min](https://github.com/vig-os/tessera/issues/363)

## Symptom

Rebasing and force-pushing a PR branch starts a new CI run but **does not cancel the one it replaced**. Both keep building.

Observed today on #360 and #361 — after a rebase, four runs were live at once for two PRs:

```
30902542139 perf/row-index-pushdown  in_progress   (head 19fc8d0 — current)
30902539943 feat/nullable-columns    in_progress   (head 80d4110 — current)
30900204959 perf/row-index-pushdown  in_progress   (head 4e21c73 — SUPERSEDED)
30900201123 feat/nullable-columns    in_progress   (head cdab19b — SUPERSEDED)
```

The two stale runs were cancelled by hand.

## Why it matters here more than usual

A cold `nix flake check` leg on a GitHub-hosted runner takes **40–52 minutes** for this stack, and the matrix is two arches. So one obsolete run occupies two runners for the better part of an hour, producing a result for a commit that no longer exists. During any rebase-heavy stretch — a stacked PR series, say — that doubles CI load for no information.

It also makes `gh pr checks` ambiguous while both runs are live.

## Fix

Add a concurrency group keyed on the workflow + ref, cancelling in progress. `.github/workflows/ci.yml`:

```yaml
concurrency:
  group: ${{ github.workflow }}-${{ github.event.pull_request.number || github.ref }}
  cancel-in-progress: true
```

`sync-issues.yml` already does this at job level (`group: sync-issues-${{ github.repository }}`, `cancel-in-progress: true`), so the pattern is established in the repo — `ci.yml` simply never got it.

One caveat worth deciding: `cancel-in-progress: true` on pushes to `main` would cancel an in-flight release-relevant build if two land quickly. Keying the group on the PR number (as above) scopes cancellation to PR runs and leaves `push`-triggered runs on `main` alone, since `github.event.pull_request.number` is empty there and the group falls back to `github.ref`.

Refs: #356
---

# [Comment #1]() by [gerchowl]()

_Posted on September 28, 2026 at 09:36 PM_

Already fixed by the devkit scaffold adoption in #442 — closing with evidence rather than duplicating it.

`.github/workflows/ci.yml` now carries a workflow-level concurrency group (lines 88-90 on `dev`):

```yaml
concurrency:
  group: ci-${{ github.workflow }}-${{ github.ref }}
  cancel-in-progress: ${{ github.event_name != 'push' }}
```

This is a slightly different shape from the one proposed above, and it handles the caveat in the
"Fix" section more directly. The proposal keyed the group on
`github.event.pull_request.number || github.ref` so that `push` runs on `main` would fall into a
separate group and not be cancelled. The scaffolded version instead keys on `github.ref` alone
(already unique per PR — `refs/pull/<n>/merge`) and makes the *cancellation itself* conditional:
`cancel-in-progress` is `false` for any `push` event. So a release-relevant build on `main` is
protected explicitly rather than as a side effect of an empty expression, and two rapid pushes to
`main` both run to completion.

The repo-wide pattern noted above (`sync-issues.yml`'s job-level group) is unchanged.

Not closing anything else here: the "stacked PRs get no CI" gap is tracked separately in #463,
since it is a trigger-filter problem rather than a concurrency one.

