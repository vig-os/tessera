---
type: issue
state: open
created: 2026-10-29T01:09:40Z
updated: 2026-10-29T07:26:29Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/688
comments: 1
labels: area:ci
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-29T08:04:01.738Z
---

# [Issue 688]: [ci: run tessera CI on the vig-os ksb-ci runners instead of GitHub-hosted runners](https://github.com/vig-os/tessera/issues/688)

Today every tessera CI job runs on GitHub-hosted runners (checked on #682's runs, 2026-10-29): `ubuntu-26.04` (Lint & Format,
Tests, Commit Messages, CI Summary, …), `ubuntu-latest` and `ubuntu-24.04-arm` (nix flake check x86_64/aarch64), and
`macos-14` / `macos-15-intel` (dist builds). The 16 GB GitHub runners are where the nix flake check OOM evictions came from
(#495), and the x86_64 leg takes ~40-90 min.

The vig-os org has a `ksb-ci` runner group (visibility: all repos). Fleet policy: `runs-on: ksb-<org>-<size>` (small
2 CPU/8 Gi for light lanes; large 8/32 Gi only for the heavy nix build/test lanes; 2 large slots per org).

## Scope
- `.github/workflows/nix-check.yml` (ours): the x86_64 nix flake check → `ksb-vig-os-large`. aarch64 stays on
  `ubuntu-24.04-arm` unless ksb has arm runners (check), because the cross-arch determinism gate needs a real aarch64 leg.
- `ci.yml` / `codeql.yml` are **devkit-scaffolded** (they regenerate on upgrade): change the runner through the devkit's
  `runner-json` / resolve-toolchain input, not by hand-editing, or the next upgrade reverts it. Light lanes → `ksb-vig-os-small`.
- macOS dist legs stay on GitHub (no macOS on ksb).
- Fork PRs never run on ksb: keep a GitHub-hosted fallback for `pull_request` from forks, or document that forks get no heavy CI.

## Acceptance
Name the exact label strings that ksb's ARC scale sets register for vig-os (verify; don't guess). Then show one PR run with
every moved job on a ksb runner (`runner_name` in the jobs API), a wall-clock comparison of the x86_64 nix leg, and no
`ubuntu-*` labels left except the documented fallbacks.
---

# [Comment #1]() by [gerchowl]()

_Posted on October 29, 2026 at 07:26 AM_

Groundwork (architect, 2026-10-29), for whoever picks this up:

- vig-os has runner group `ksb-ci` (id 5, visibility all). No runners are registered at rest, which is expected for ARC scale sets: runners register only while a job is waiting. The seat's kube service account can't list `autoscalingrunnersets` (Forbidden), so the vig-os scale-set name is **not yet verified**. By the fleet pattern it would be `ksb-vig-os-small` / `ksb-vig-os-large`. Verify it before wiring: run a one-job probe workflow on the PR branch with `runs-on` set to that label, and record `runner_name` from the jobs API. If nothing picks the job up within a few minutes, ask the ksb admin for the scale-set names. Don't guess.
- The devkit routing mechanism, as exo-fleet's `docs/hosts/bifrost/ci-vm-runbook.md` (~L459-467) does it: devkit lanes go through `.vig-os` `DEVKIT_CI_RUNNER=ksb-<org>-large`, and `resolve-toolchain` goes through the repo variable `DEVKIT_CI_RESOLVE_RUNNER=ksb-<org>-small`. A literal `runs-on: ksb-…` in a repo-owned workflow also needs an entry in `.github/actionlint.yaml` `self-hosted-runner.labels`, with a reason, as exo-fleet's does.
- Examples of repo-owned lanes on ksb: `MorePET/visionx-rs` `.github/workflows/ci-extension.yml` (`ksb-morepet-small` / `-large`).

