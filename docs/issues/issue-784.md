---
type: issue
state: open
created: 2026-11-09T03:44:13Z
updated: 2026-11-09T03:44:13Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/784
comments: 0
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-09T09:04:19.290Z
---

# [Issue 784]: [chore(deps): Renovate is rate-limited by 11 unlanded PRs; group the vortex crates and gate them on the fork pin](https://github.com/vig-os/tessera/issues/784)

Renovate saw vortex 0.93 (Dependency Dashboard #515) but never opened a PR, and dependency updates have piled up unlanded since October. Root causes:

1. **The PR concurrency cap is saturated.** `config:recommended` limits Renovate to 10 open PRs (and 2 per hour). Eleven are open, most from 2026-10-12/19 (#558, #560, #561, #640, #641, #643, #644, #645, #646, #649, #689 [security]). Every newer update, including all of vortex, sits under "Rate-Limited" on #515. The open PRs were never landed: a process gap, not a Renovate bug.
2. **The vortex crates aren't grouped.** The dashboard lists 7 separate per-crate updates (vortex-array, -btrblocks, -buffer, -file, -io, -layout, -session). All 27 workspace crates must move together, so any single-crate PR fails.
3. **The fork `[patch]` is invisible to Renovate.** vortex comes through a git `[patch]` pinned by full SHA (#480, #758). A version bump alone makes cargo silently ignore the patch (the 0.75 tag trap). Renovate must not bump vortex independently of the patch.

**Acceptance**
- Land or close every open Renovate PR. Each one rebased, CI green, fresh-reviewed per the normal path; #689 [security] first. Superseded ones are closed with the reason.
- `.github/renovate-default.json` (ours): a `packageRules` group for `/^vortex/` (all crates in one PR). While the fork pin exists, either disable vortex updates with a link to #480/#635, or set `dependencyDashboardApproval: true` so they appear only for a deliberate upgrade. Record the choice.
- Decide whether to keep or raise `prConcurrentLimit`, given the CI capacity (ksb ARC, #772), and record why.
- Process: the architect lands Renovate PRs on the weekly schedule. Nothing sits for more than a week without a disposition.

Refs: #635, #480, #515

