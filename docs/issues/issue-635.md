---
type: issue
state: open
created: 2026-10-18T23:50:37Z
updated: 2026-10-19T00:00:07Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/635
comments: 1
labels: dependencies, priority:medium, area:core
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-19T07:17:41.519Z
---

# [Issue 635]: [build(deps): plan the vortex 0.75 → upstream jump (fork is 13 releases behind, no bumps flow)](https://github.com/vig-os/tessera/issues/635)

## State (2026-10-19)
- Pinned: vortex **0.75** via `[patch.crates-io]` → `gerchowl/vortex@608d8a1` (0.75 + 3 commits, 2026-09-29; PR #481).
- Upstream: **0.88.0** (2026-10-08); the fork is 3 ahead / **~1128 behind** `develop`. Nothing bumps it: Renovate has never opened a vortex PR
  (the SHA patch freezes it), so the gap grows silently.
- Upstream fixes: vortex-data/vortex#10122 (#472 canonical NaN sum) **merged** 2026-10-09, after 0.88.0, so it ships in the next release;
  vortex-data/vortex#10121 (#468 patch-indices `is_sorted` stats caching) **open, REVIEW_REQUIRED, idle since 2026-10-13**.

## Why not track upstream continuously
Sealed table bytes are Vortex encoder output: any bump can move `content_hash`/`manifest_hash` and the corpus goldens, and can
re-register encodings (ADR-0057 Gate A/B). Every bump is a **format event** (corpus regeneration + cross-arch determinism evidence),
so per-release churn is wrong. Freezing is also wrong: API drift compounds and we miss fixes (security, the #628 compact/zstd
work, wasm).

## Plan
1. Get #10121 reviewed or merged upstream (the only thing keeping the fork alive).
2. **One planned jump** to the first upstream release that contains both fixes (expected 0.89), closing #480 and deleting the fork:
   a corpus-regen PR with cross-arch (x86_64 + aarch64) byte-equality evidence, the Gate A/B feature snapshots refreshed and their deltas
   explained, and the API port. Sequence it with the #628 compression spike (the zstd decision depends on the vortex version).
3. Until then, change the fork only for security advisories (cargo-deny reports them).
4. Add a scheduled report (monthly CI job or a Renovate rule in dashboard-only mode) that shows the latest vortex release and whether
   #10121/#10122 are included, so the drift is visible.

---

# [Comment #1]() by [gerchowl]()

_Posted on October 19, 2026 at 12:00 AM_

Gate audit (2026-10-19): forward determinism is guarded (golden hashes over the encoded Vortex bytes on both arches; Gate A/B; seal-profile-determinism), but backward read, rich committed table fixtures and size are NOT. Order before the jump: 636 (backward-read value goldens) → 637 (size ratchet) → 638 (canary + changelog review checklist) → the planned jump. Also: Gate B's vortex-*.txt snapshots embed the fork rev, and table.rs:1640's hardcoded 2772/2916/2444 lengths will move on any bump.

