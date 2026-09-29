---
type: issue
state: closed
created: 2026-09-28T18:52:35Z
updated: 2026-09-28T20:19:59Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/450
comments: 1
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-09-29T07:52:32.469Z
---

# [Issue 450]: [fix(ci): devkit branch-name gate rejects `fix/`, `feat/`, `ci/` — contradicts .pre-commit-config.yaml and blocks PRs](https://github.com/vig-os/tessera/issues/450)

The devkit scaffold adopted in b7cba24 (#442) added a **Validate branch name** step to the
`Commit Messages` job whose type list contradicts this repo's own documented convention. Every PR on a
`fix/`, `feat/`, `perf/`, `polish/`, `spike/`, `build/`, `ci/`, `style/` or `land/` branch now fails CI.

Observed on PR #447 (branch `fix/396-nifti-decoder-bugs`):

```text
##[error]Branch name 'fix/396-nifti-decoder-bugs' does not follow the convention
<type>/<issue>-<summary> (types: feature,bugfix,hotfix,release,docs,test,refactor)
or chore/<summary>.
```

## The conflict

Three places disagree about the branch-type list:

| Source | List | Allows `fix/`? |
|---|---|---|
| `.pre-commit-config.yaml` (local `branch-name` hook) | `chore\|feat\|feature\|fix\|bugfix\|hotfix\|perf\|polish\|spike\|refactor\|docs\|test\|build\|ci\|style\|land` | **yes** |
| `docs/COMMIT_MESSAGE_STANDARD.md` | lists `fix` as a type (line 42) | **yes** |
| `.github/actions/resolve-toolchain/action.yml` `DEFAULT_BRANCH_TYPES` | `feature,bugfix,hotfix,release,docs,test,refactor` | **no** |

So a branch passes the local hook on commit and then fails in CI — the worst ordering, because the
developer only learns after pushing.

The local hook's own comment already argued this exact case when the list was last widened:

> The type list is derived from branch names actually used on merged PRs (checked against
> `gh pr list --state merged`), not from an aspirational convention: the previous pattern rejected
> `feat/`, `fix/`, `perf/`, `polish/`, `land/` and `spike/` — i.e. **14 of the last 30 merged PR
> branches** … A gate that no real branch can satisfy is not a gate; it just teaches everyone to pass
> `--no-verify`, which then bypasses the *other* gates in this file too.

The devkit default re-introduces precisely the pattern that reasoning rejected.

## Fix

The devkit already supports an override; this repo just has no `.vig-os` file, so CI falls back to the
stock list. Create one at the repo root:

```sh
# .vig-os — vigOS devkit configuration (see .github/actions/resolve-toolchain).
# DEVKIT_BRANCH_TYPES mirrors the `branch-name` hook in .pre-commit-config.yaml. Keep the two in
# lockstep: the local hook and the CI gate must accept exactly the same set, or a branch passes on
# commit and fails after push.
DEVKIT_BRANCH_TYPES=chore,feat,feature,fix,bugfix,hotfix,perf,polish,spike,refactor,docs,test,build,ci,style,land
```

Every entry matches the action's `^[a-z][a-z0-9]*$` validation, so it resolves rather than warning and
falling back. Confirmed by reading `.github/actions/resolve-toolchain/action.yml` (the
`DEVKIT_BRANCH_TYPES` → `branch-types` output mapping, ~line 308).

`DEVKIT_COMMIT_TYPES` appears to need no override — the commit-*message* validation passed on #447 with
`fix(ingest):` and `test(ingest):` subjects; only the branch-*name* step failed.

## Acceptance criteria

- [ ] `.vig-os` sets `DEVKIT_BRANCH_TYPES` to the same set as the `branch-name` hook.
- [ ] A `fix/<issue>-<summary>` branch passes the `Commit Messages` job.
- [ ] The two lists carry a comment pointing at each other, so the next edit keeps them in lockstep.
- [ ] Decide whether `docs/COMMIT_MESSAGE_STANDARD.md` should state the branch-type list explicitly —
      right now it documents *commit* types and the branch convention is only encoded in two configs.

## Note

This blocks any PR not on one of the seven stock types, so it is worth landing ahead of the queue
rather than inside whichever PR happens to trip over it. Found from PR #447 (#396); not caused by it.

## References

- b7cba24 / #442 (devkit scaffold adoption) · PR #447 (where it surfaced)
- `.github/workflows/ci.yml` (~line 328) · `.github/actions/resolve-toolchain/action.yml` (~line 308)
- `.pre-commit-config.yaml` (the `branch-name` hook and its rationale) · `docs/COMMIT_MESSAGE_STANDARD.md`

---

# [Comment #1]() by [gerchowl]()

_Posted on September 28, 2026 at 06:54 PM_

**Correction to the "Fix" section above** — thanks to @gerchowl for catching it. `.vig-os` **does** exist
at the repo root (14 KB, `DEVKIT_VERSION=1.16.0`); the key is present but **empty**:

```sh
DEVKIT_BRANCH_TYPES=
```

An empty value falls through to `DEFAULT_BRANCH_TYPES` in `resolve-toolchain`, which is why the gate used
the stock seven. So the fix is *filling in an existing key*, not creating a file. My original text said
the repo "has no `.vig-os` file" — wrong; I grepped with `--include=*.yml|*.toml|*.env|*.txt` filters,
which cannot match an extensionless name.

Two things the file's own documentation adds, worth recording because they affect how the fix should be
applied:

1. **The key drives three surfaces, not one.** Per its comment, `DEVKIT_BRANCH_TYPES` drives "the local
   `no-commit-to-branch` hook, the flake-generated consumer surface (read by `flake.nix` at eval time),
   and CI's branch-name gate **from this one key**" — but the hook pattern is *"realized at scaffold
   time (an anchored render)"*, whereas CI reads the key at runtime via `resolve-toolchain`. Since
   `DEVKIT_DRIFT_CHECK=false` here (#441's release.yml filename collision) and no re-scaffold runs in
   CI, setting the key fixes the CI gate immediately **without** re-rendering
   `.pre-commit-config.yaml`. That is the desired outcome — the local hook already carries the wide,
   deliberately-widened list — but it means the two stay in sync only by hand until a re-scaffold
   happens, and a future `init-workspace.sh --force` will render the hook pattern *from this key*. All
   the more reason to make the key's value exactly match the hook's current list.

2. **Keep `release` in the list.** The file notes the scaffold "prints a notice when `release` is
   dropped", and `release/X.Y.Z` is release-plz's namespace. The `chore/<summary>`, `renovate/*` and
   `worktree/<issue>` clauses are never knob-driven, so they need no entry.

Being handled in PR #451 (`bugfix/450-devkit-branch-types`).

