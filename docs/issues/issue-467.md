---
type: issue
state: open
created: 2026-09-28T21:40:00Z
updated: 2026-09-29T06:53:15Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/467
comments: 0
labels: none
assignees: none
milestone: backlog / research
projects: none
parent: none
children: none
synced: 2026-09-29T07:52:28.569Z
---

# [Issue 467]: [deps: both Renovate and Dependabot are configured, but only Dependabot runs](https://github.com/vig-os/tessera/issues/467)

## Observation

This repo carries configuration for **two** dependency bots covering the same ecosystems:

- `renovate.json` (devkit-scaffolded) — `"enabledManagers": ["github-actions", "pep621", "npm"]`,
  extending `github>vig-os/tessera//.github/renovate-default`, which exists.
- `.github/dependabot.yml` (hand-written, predates the devkit scaffold) — `github-actions`, `pip`
  (`/tessera/bench/ecosystems`), and `npm`, all targeting `dev`.

Only Dependabot is actually running. Evidence that Renovate is not installed on the repo:

- no `renovate/*` branch has ever existed (`git branch -r` shows only `dependabot/*`),
- no Dependency Dashboard issue (`gh issue list --search "Dependency Dashboard"` is empty),
- every bot PR in the repo's history is authored by `app/dependabot`.

So `renovate.json` is currently inert config.

## Why it is worth resolving rather than leaving

1. **It hides which bot is authoritative.** Someone tuning `renovate.json` to change grouping or
   scheduling would see no effect at all, with nothing to indicate why.
2. **If Renovate is ever installed, both bots raise PRs for the same updates.** The `github-actions`
   and `npm` managers overlap exactly with Dependabot's ecosystems.
3. **devkit is built around Renovate.** Its `ci.yml` branch-name allowlist has a `renovate/.+` clause
   and nothing for `dependabot/**`, which is precisely the breakage in #466 — every Dependabot PR
   fails the `Commit Messages` job. Standardising on Renovate would make that class of problem
   disappear without a local `ci.yml` patch that a devkit upgrade will overwrite.

## Options

1. **Adopt Renovate, retire `dependabot.yml`** — the devkit-standard path, and it resolves #466
   structurally. Needs the Renovate App installed on the repo (an org/repo settings action), and the
   `pip` manager for `/tessera/bench/ecosystems` re-expressed as `pep621` (already in
   `enabledManagers`). Dependabot *security* alerts are independent of `dependabot.yml` and would be
   unaffected either way.
2. **Keep Dependabot, delete `renovate.json`** — smallest change, matches what actually runs today.
   Keeps the local `ci.yml` allowlist patch (and its upgrade fragility) until vig-os/devkit#1755 lands.
   Note the scaffold would re-ship `renovate.json` on a future upgrade unless `renovate` is added to
   `DEVKIT_FEATURES_DISABLED`.

Recommending (1), but it is a settings-level decision, so filing rather than acting. Not blocking
#466, which is being unblocked with the local patch now either way.
