---
type: issue
state: open
created: 2026-09-29T08:32:24Z
updated: 2026-09-29T08:32:25Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/505
comments: 0
labels: none
assignees: none
milestone: devkit release adoption
projects: none
parent: none
children: none
synced: 2026-09-30T07:57:10.278Z
---

# [Issue 505]: [release-plz under merge-commit-only: changelog granularity and bump votes now come from intermediate commits](https://github.com/vig-os/tessera/issues/505)

## What changed

Squash merging was disabled on this repository at **06:08 today**; merge commits are now the only
method. That changes what reaches `dev`, and `release-plz` derives the workspace version and
`CHANGELOG.md` from `dev`'s history — so it changes the release inputs. **Nothing is failing**; this is
about what the next cut will look like.

## Two things are fine

Checked rather than assumed:

- **PR merge commits carry the PR title**, not `Merge pull request #N from …`. #498 landed as
  `ci(nix): --cores 2 against the OOM evictions … (#498)`. So the PR-level history looks exactly as it
  did under squash, and nothing chokes on the subject.
- **No gate objects.** Only `release-plz.yml` runs on push to `dev`; everything else is main-only,
  tag-only, PR-only or scheduled. `validate-commit-range` sits behind
  `if: github.event_name == 'pull_request'` and skips merge commits anyway. release-plz's last runs are
  all `success`.

## Two things changed, and both feed the release

**1. Every intermediate commit now reaches `dev`.** Under squash, one conventional commit per PR. Now
the branch's full history lands. The commit-message gate keeps these clean — **0 of the last 40
non-merge commits on `dev` are non-conventional** — so this is not garbage, it is *granularity*:

- the changelog gains an entry per intermediate commit rather than one per PR;
- **each intermediate commit votes on the version bump.** A PR titled `fix:` containing an intermediate
  `feat:` now argues for a minor bump where the squash would have said patch. That is the part worth
  deciding deliberately rather than discovering at the cut.

**2. Branch-update merge commits land too, and those are not conventional.** One is already on `dev`:

```
ee274a8 Merge remote-tracking branch 'origin/dev' into ci/495-nix-check-timeout-headroom
```

Mine, from updating a PR branch before the guidance changed. The team default has since flipped to
rebase-onto-dev, which prevents new ones, but it does not remove the one already there and cannot
prevent every case.

## Options

`release-plz.toml` is deliberately minimal and has **no `[changelog]` section**, so git-cliff's defaults
apply. Worth evaluating, in rough order of effort:

1. **`commit_parsers` / filtering** in a `[changelog]` block — skip merge commits explicitly, and decide
   whether intermediate commits are grouped or dropped. Smallest change, keeps release-plz as-is.
2. **A PR-grouped changelog**, if release-plz's current version supports grouping entries by pull
   request. That would restore the one-entry-per-PR shape the squash flow gave for free. I have not
   verified this option's availability in the pinned version — it needs checking against the release-plz
   docs before anyone relies on it.
3. **Do nothing deliberately**, and accept a finer-grained changelog. Defensible — arguably more
   informative — but the bump-vote behaviour should still be a conscious choice.

Whichever way, the version-bump question should be settled before the alpha is cut, since that is the
first moment the changelog and the computed version actually matter. `CHANGELOG.md` is still an empty
Keep-a-Changelog skeleton, so nothing has been generated yet — this is the cheap moment to decide.

## Why this milestone

#441's devkit release train would replace release-plz entirely, at which point this becomes moot. So the
cheapest resolution may be to decide nothing here and let the devkit adoption settle it — but that only
works if the alpha is cut *after* that adoption. If the alpha comes first, option (1) is the small,
reversible fix.

Refs #441, #498
