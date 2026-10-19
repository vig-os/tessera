---
type: issue
state: open
created: 2026-10-19T00:00:06Z
updated: 2026-10-19T00:00:06Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/638
comments: 0
labels: feature, dependencies, priority:medium, area:ci
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-19T07:17:40.064Z
---

# [Issue 638]: [ci(deps): vortex canary on every upstream release + agent changelog review checklist](https://github.com/vig-os/tessera/issues/638)

Part of #635. Owner direction (2026-10-19): automatic visibility of every upstream Vortex release, deliberate landing.

1. **Canary (CI, scheduled + workflow_dispatch):** resolve the newest upstream `vortex-*` release, replace the fork `[patch]` with that
   release (or the fork rebased onto it while #10121 is unreleased), port nothing, and run the full `nix flake check --keep-going` on
   x86_64 + aarch64. File or update ONE issue (label `vortex-canary`) with the verdict: build/API break / bytes stable (all goldens equal) /
   goldens moved (which fixtures, which arch) / backward-read failed / size moved (from the size ratchet). Never opens a merge-ready PR
   by itself.
2. **Changelog review checklist** (`docs/maintenance/vortex-bump-review.md`, versioned): the questions an agent answers from the upstream
   release notes and merged PRs between our pin and the candidate: format and encoding changes or re-registrations (FSST/ALP/Pco/zstd-compact),
   layout or footer; determinism-relevant changes (stats, float/NaN, sortedness/patches, writer threading or order, `debug_assertions`
   side effects: the #468/#472 class); whether our upstream fixes (#10121/#10122) shipped and which local workarounds can go;
   security advisories; opportunities (#628 zstd/compact, wasm, nullable/stats #627, range reads); deprecations and API breaks.
   The canary issue is dispatched to an agent with this checklist as its brief; it comments its findings and files only confirmed issues.
3. Landing stays human-gated: bytes-stable and review-clean → an ordinary bump PR; bytes moved or the review flags something → the planned format
   event in #635.
Depends on the backward-read corpus and the size ratchet (sibling issues), otherwise the canary can report green on a broken read or a
2× size regression.

