---
type: issue
state: closed
created: 2026-11-02T06:08:49Z
updated: 2026-11-02T08:49:57Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/725
comments: 0
labels: feature
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-02T08:51:35.034Z
---

# [Issue 725]: [ci(nix): run the aarch64 flake check nightly on dev and on releases, not per PR](https://github.com/vig-os/tessera/issues/725)

**Owner decision (2026-11-02):** take the aarch64-linux `nix flake check` leg off the per-PR path, to cut PR latency.
It is the slowest leg: 46–82 min hosted. It keeps running **nightly on dev (schedule), on release cuts, and on manual
`workflow_dispatch`**. It will run per release once the first real releases start.

## What is being traded away (state it in nix-check.yml's header)
Since #688 the ksb x86_64 leg runs Nix with `sandbox = false` (exo-pet/exo-fleet#1081). The hosted aarch64 leg was the
**only sandboxed** flake check, and the **only cross-arch determinism check** (#472/#468 class: S15
writer-determinism across arches). With this change, both are checked nightly instead of per PR. A regression is
caught within a day and is traceable to that day's merges.

## Requirements
- `nix-check.yml`: the aarch64 matrix leg runs only on `schedule` (nightly, on dev), `workflow_dispatch` and the
  release path. Choose the release hook: a tag push or `release-nix.yml`. On `pull_request`/`push`, x86_64 only.
- Branch protection and the aggregate `nix flake check` context must still pass on PRs with one leg. Check that the
  aggregate requires exactly the legs that run for the event, BY NAME, and never vacuously (see #722/#723).
- The nightly failure must be visible: on failure, file or update an issue (label `bug`, `area:ci`), not just a red
  run nobody sees.
- Update FEATURE-MATRIX / docs where they claim cross-arch determinism is checked per PR.
- Fail-before proof: show from the workflow logic, e.g. an `act`-free evaluation or the GitHub run list, that a PR
  run has no aarch64 leg and a `workflow_dispatch` run has one.
**Order:** after PR #723 (#722) merges, since both edit nix-check.yml.

