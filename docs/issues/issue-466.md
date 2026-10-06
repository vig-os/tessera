---
type: issue
state: open
created: 2026-09-28T21:39:40Z
updated: 2026-10-05T09:40:31Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/466
comments: 1
labels: none
assignees: none
milestone: 0.1.0-alpha.2
projects: none
parent: none
children: none
synced: 2026-10-06T08:33:16.314Z
---

# [Issue 466]: [ci: drop the local dependabot/** branch-name allowlist patch once the devkit supports extra branch patterns (devkit#1755)](https://github.com/vig-os/tessera/issues/466)

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

---

# [Comment #1]() by [gerchowl]()

_Posted on October 5, 2026 at 09:40 AM_

Re-scoped (2026-10-05). **The symptom is currently unreachable:** `automated-security-fixes` is `{"enabled":false}` and `.github/dependabot.yml` was retired by #512, so Dependabot can't open a PR by either route (it still *reports* alerts, since `vulnerability-alerts` is 204). There are no open `dependabot/**` branches.

The issue stays open because the local `ci.yml` patch (`ALLOWED+="|^dependabot/.+$"`) is the only thing that would let a first bot PR pass. That would be the case if security updates are switched back on, which is plausible while `thrift` remains an alert no bot can fix in range. `ci.yml` is devkit-scaffolded and regenerates on upgrade: `DEVKIT_UPGRADE_EXCLUDE` lists only `release.yml`. So **the patch will be silently dropped at the next devkit upgrade.** There's no devkit knob: `DEVKIT_BRANCH_TYPES` only feeds the `<type>` alternation, so it can't match `dependabot/github_actions/...`.

Standing action: when the devkit gains an extra-branch-pattern input (devkit#1755, or a dedicated issue), move the `dependabot/` clause there and drop the local patch. Until then, re-check this patch after every devkit upgrade.

