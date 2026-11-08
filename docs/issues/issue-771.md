---
type: issue
state: open
created: 2026-11-08T03:03:41Z
updated: 2026-11-08T03:03:41Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/771
comments: 0
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-08T08:33:51.432Z
---

# [Issue 771]: [chore: adopt devkit 1.19.0 (supersedes #557)](https://github.com/vig-os/tessera/issues/771)

Adopt devkit **1.19.0** (latest, 2026-11-08); dev pins 1.17.0. This supersedes the bot's PR #557 (1.18.0), which fails CI at `Resolve toolchain` (run 41458295205) and skipped the flake bump: "input 'vigos' is pinned to '1.17.0' (never auto-bumped; set DEVKIT_FLAKE_PIN_ADVANCE=true …)".

**Acceptance**
- `.vig-os` `DEVKIT_VERSION=1.19.0`, re-scaffolded with the devkit's own installer/upgrade path; no hand-edits to managed files. `scaffold-drift` is green.
- The flake input `vigos` is bumped to 1.19.0 in `flake.nix`/`flake.lock` (devkit#1756: the upgrade misses it). Decide whether to set `DEVKIT_FLAKE_PIN_ADVANCE=true` and record the reason.
- The root cause of #557's `Resolve toolchain` failure is diagnosed and fixed at the source (a tessera config or a devkit bug filed upstream), not worked around.
- Local patches kept across the re-scaffold are each re-verified: the dependabot allowlist (#466), the stacked-PR trigger (#463), anything in `.vig-os` comments.
- Release notes for 1.17.0→1.19.0 are reviewed for breaking/consumer-action items, and each one is handled or recorded.
- `nix flake check` green on CI (x86_64 ksb leg); feature-snapshots unaffected or regenerated with proof.
- #557 closed as superseded once this merges.

