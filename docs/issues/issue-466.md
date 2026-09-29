---
type: issue
state: open
created: 2026-09-28T21:39:40Z
updated: 2026-09-29T06:52:56Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/466
comments: 0
labels: none
assignees: none
milestone: 0.1.0-alpha.2
projects: none
parent: none
children: none
synced: 2026-09-29T07:52:28.877Z
---

# [Issue 466]: [ci: every Dependabot PR fails the Commit Messages job — branch-name gate has no dependabot/** clause](https://github.com/vig-os/tessera/issues/466)

## Symptom

Both open Dependabot PRs (#443, #444) fail exactly one check, `Commit Messages`. Everything else on
them is green, including `nix flake check` on both arches and `CodeQL Analysis (actions)`.

## Root cause

They fail at the **`Validate branch name`** step, before the commit-message validator runs at all:

```
##[error]Branch name 'dependabot/github_actions/dev/actions-minor-patch-64e3431685' does not follow
the convention <type>/<issue>-<summary> (types: feature,bugfix,hotfix,release,docs,test,refactor)
or chore/<summary>.
```

The allowlist in `ci.yml` covers Renovate's namespace but not Dependabot's:

```bash
ALLOWED+="|^renovate/.+$"          # Renovate is covered
# nothing for dependabot/**
```

Dependabot's `dependabot/` prefix is not configurable (`pull-request-branch-name.separator` only
changes the separator), so this cannot be fixed from `dependabot.yml`.

## It is NOT the `Refs:` line

Worth recording, because that is the obvious first guess and it is wrong. The commit-message half
already handles bots correctly:

- `validate_commit_range.py` skips `…[bot]` authors (`BOT_AUTHOR_SUFFIX`), so `ci(deps): bump …` with
  no `Refs:` line is accepted by design.
- `validate_title` marks every approved type Refs-optional, so the PR title passes too.

So `DEVKIT_REFS_OPTIONAL_TYPES` / `DEVKIT_REFS_POLICY` would be a no-op here.

Also **not** the cause of #445 (`chore: adopt devkit 1.17.0`): `chore/devkit-1-17-0` matches the
existing `chore/<slug>` clause, and that PR fails `Lint & Format` for an unrelated reason — devkit
1.17.0 adds a `shellcheck-composite-actions` hook while `flake.nix` still pins `devkit/1.16.0`, so
the binary is missing from the dev-shell (upstream: vig-os/devkit#1756).

## Fix

Upstream in devkit, since `ci.yml` is devkit-managed: vig-os/devkit#1755.

In the meantime this repo patches the allowlist locally. `DEVKIT_DRIFT_CHECK=false` already (see #441),
so the patch does not fail the drift gate — but `devkit-upgrade.yml` regenerates `ci.yml`, so **the
patch will be dropped on the next devkit upgrade** unless devkit#1755 has landed. Deliberately not
adding `ci.yml` to `DEVKIT_UPGRADE_EXCLUDE`: that freezes the entire workflow and forfeits every
future CI improvement to preserve a one-line clause. Keeping this issue open as the reminder to drop
the local patch once devkit#1755 lands.

Related: #467 — this repo has both `renovate.json` and `.github/dependabot.yml` configured for the
same ecosystems, and standardising on Renovate would make this class of problem go away, since
devkit's allowlist already covers `renovate/*`.

