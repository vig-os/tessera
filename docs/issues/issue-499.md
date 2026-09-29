---
type: issue
state: open
created: 2026-09-29T06:06:55Z
updated: 2026-09-29T06:52:52Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/499
comments: 0
labels: none
assignees: none
milestone: 0.1.0-alpha.2
projects: none
parent: none
children: none
synced: 2026-09-29T07:52:23.125Z
---

# [Issue 499]: [ci: the local branch-name hook and the CI branch-name gate disagree — one accepts <type>/<slug>, the other requires an issue number](https://github.com/vig-os/tessera/issues/499)

## Symptom

A branch can pass **every local gate** and fail only in CI, on branch naming.

`ci/nix-check-timeout-headroom` was accepted by `prek`/pre-commit locally, then rejected by the
`Commit Messages` job:

```
##[error]Branch name 'ci/nix-check-timeout-headroom' does not follow the convention
<type>/<issue>-<summary> (types: chore,feat,feature,fix,bugfix,hotfix,release,perf,polish,…)
```

That cost a full CI cycle and a PR (#496 → #498), because renaming a branch closes its PR rather than
carrying it.

## The two patterns differ

**Local** — `.pre-commit-config.yaml`, `no-commit-to-branch`. The allowed form is
`(chore|feat|feature|fix|bugfix|hotfix|perf|polish|spike|refactor|docs|test|build|ci|style|land)/[a-z0-9][a-z0-9._-]*`
— a slug, with **no issue number required**. The comment there is explicit about it:

> The issue number is OPTIONAL — put it in the summary (`feat/349-log-axis-transform`) when there is
> one. It is NOT required in the branch name because `.gitmessage` already makes every commit carry
> `Refs: #<issue>`; requiring it in both places bought nothing and blocked most real work.

**CI** — the devkit-managed `ci.yml` `Validate branch name` step:

```bash
ALLOWED+="|^(${TYPES_ALTERNATION})/[0-9]+-[a-z0-9]+(-[a-z0-9]+)*$"
```

A numeric issue segment is **mandatory** for every typed branch. `chore/<slug>` is the only
number-free typed form it accepts.

So the two gates encode opposite decisions about the same question, and the local one states a
rationale for its choice that CI silently overrides.

## Why it matters beyond one wasted cycle

`nix-check.yml`'s header argues the value of running one command locally and in CI is that there is no
"passes locally / fails in CI" drift. That argument applies to the naming gates too, and here it is
broken: the local hook is strictly more permissive, so it cannot catch what CI will reject. A gate that
only fires after a 45-minute CI leg is a much worse gate than one that fires at commit time.

It also punishes exactly the branches the local comment set out to allow: issue-less maintenance work
under a type other than `chore` (a `ci/` tidy-up, a `docs/` fix) is legal locally and impossible in CI.

## Options

1. **Make the local hook match CI** (require `<type>/<issue>-<summary>`, keep `chore/<slug>` free). The
   local gate then catches it at commit time, which is where it belongs. But it overrides the local
   comment's deliberate decision, so that reasoning should be reconsidered rather than just deleted.
2. **Make CI match the local hook** (accept `<type>/<slug>` for every type, not just `chore`). This is
   upstream in devkit — `ci.yml` is managed — so it needs a devkit change or a local patch like the
   `dependabot/**` one (#466, upstream vig-os/devkit#1755). It is also arguably the better rule, for
   the reason the local comment gives: `Refs:` already carries the issue, so requiring it twice adds
   nothing.
3. Document the stricter rule prominently and accept the drift. Weakest option — it leaves a gate that
   can only fail late.

Recommending **(2)**, with **(1)** as the immediate stopgap since it is in-repo and takes effect at once.
Whichever way it goes, the two patterns should be derived from one source rather than maintained in
parallel — they have already diverged once.

Refs #466, #498
