---
type: issue
state: open
created: 2026-09-29T00:04:10Z
updated: 2026-09-29T06:53:13Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/480
comments: 1
labels: none
assignees: none
milestone: backlog / research
projects: none
parent: none
children: none
synced: 2026-09-29T07:52:26.695Z
---

# [Issue 480]: [build(deps): remove the vortex fork pin once both determinism fixes are released upstream](https://github.com/vig-os/tessera/issues/480)

## What

tessera pins `vortex` to a patched fork by git SHA to carry two determinism fixes that are not yet in
a published release. **This issue is the reminder to remove that pin.**

Remove it when a released `vortex` contains BOTH of:

- **#468** — the `debug_assertions`-gated `is_sorted` assert in `vortex-array/src/patches.rs` caches
  `Stat::IsSorted` on the patch indices. Cached statistics are serialised, so the written bytes
  depended on the build profile (debug wrote 24 bytes more).
- **#472** — `sum_float_all` skips NaN, so a column holding both `+inf` and `-inf` evaluates
  `inf + -inf`, and the resulting default NaN differs by architecture (x86_64 `0xfff8…`, aarch64
  `0x7ff8…`). Sums are persisted as `Stat::Sum`, so the written bytes depended on the writing host.

Both are the same underlying defect class: **write-time statistic computation leaking into the
serialised bytes.**

## How to remove it

1. Bump `vortex` to the first release containing both fixes.
2. Delete the `[patch.crates-io]` block from `tessera/Cargo.toml` (all 27 vortex crates) and the
   `[sources] allow-git` entry for the fork in `tessera/deny.toml`.
3. Re-run Gate B (`scripts/feature-snapshots.sh tessera/tests/feature-snapshots`) — the snapshots will
   move back from a `git+` source to a registry source. Expect source-line churn only; **no feature
   may change**.
4. Confirm `gen_corpus` output stays byte-identical and the ingest corpus is unchanged. The two fixes
   were validated as golden-preserving when the pin went in, so removing it must be golden-preserving
   too. If a golden moves, the released fix differs from the forked one — investigate before
   regenerating.
5. Confirm the `known_limitation_468_*` flip still passes: both profiles must agree.
6. Delete the fork branch `gerchowl/vortex:tessera/0.75.0-patched`.

## Why a fork pin rather than waiting

The two fixes are prerequisites for the generic-ingest corpus: without them its fixtures either
assert host-dependent values or have to be narrowed to avoid the pathological shapes, which is what
this repo was doing. The corpus is the thing that proves the format is deterministic, so weakening it
to work around an upstream bug defeats its purpose.

## Upstream

Filed against vortex-data/vortex (see the PR that adds the pin for the issue and PR links). The fork
branch is `0.75.0` plus the two fixes plus one non-upstream commit that sets the workspace version to
`0.75.0` — the tag carries the placeholder `0.1.0`, so without it `[patch.crates-io]` is silently
IGNORED rather than failing loudly.

Refs #468, #472
---

# [Comment #1]() by [gerchowl]()

_Posted on September 29, 2026 at 12:32 AM_

## Fork durability, and one thing deliberately left undone

**The pinned branch is now protected.** `gerchowl/vortex:tessera/0.75.0-patched` has `lock_branch`,
`allow_force_pushes: false`, `allow_deletions: false` and `enforce_admins: true`. A deleted or
rewritten branch can let GitHub garbage-collect the pinned commit, which would break fresh builds with
a lockfile that still looks perfectly valid — the failure would appear as an unfetchable rev, long
after the cause. Verified the branch tip still equals the pinned
`608d8a1162cf40dbea70f8dd72781ec243754282`.

`enforce_admins` is deliberate: it locks the branch against me too, which is the point. The pin is only
as durable as the weakest thing that can move it.

**Mirroring to the vig-os org is available if wanted.** The pin currently depends on a personal
account's fork staying reachable. Mirroring `tessera/0.75.0-patched` to a vig-os-owned repository and
re-pinning to that URL would remove that dependency. Not done unilaterally — it is an org-level
decision about where a build-critical dependency lives, and re-pinning means a new rev, a lockfile
update and a Gate B regeneration. Worth doing if this pin is expected to live for more than a release
or two; skippable if the upstream PRs land soon.

**The version-surgery commit is NOT signed, on purpose.** Signing it — DCO trailer or GPG — rewrites
the commit and therefore changes the SHA, which is exactly what the pin must not do (it would cascade
into `Cargo.toml`, the lockfile, all 8 Gate B snapshots and the PR body). More to the point, DCO exists
to certify the right to submit a patch *upstream*, and that commit is explicitly never upstreamed: it
only sets the workspace version so `[patch.crates-io]` resolves at all. The two commits that DO go
upstream are both signed (`Signed-off-by: Lars Gerchow <lars.gerchow@gmail.com>`).

## Upstream status

| defect | issue | PR |
|---|---|---|
| #468 | vortex-data/vortex#10119 | vortex-data/vortex#10121 |
| #472 | vortex-data/vortex#10120 | vortex-data/vortex#10122 |

Both rebased onto vortex's `develop` and re-verified as still present there before filing. Note #10122
fixes **two** sites on develop — a newer `sum_v2` finalises floats through its own path — so the
"released version containing both fixes" check in the removal steps above should confirm the canonical
NaN comes back from *both* `sum` and `sum_v2`.

## Guards that will notice if this pin is dropped early

Both added on the pin PR (#481) and both run under debug and release in `flake.nix`:

- `full_span_int_container_bytes_are_build_config_independent` — #468. Fires on **either** arch, since
  that defect's axis is the build profile.
- `float_sum_stat_is_canonical_nan_not_the_platform_default` — #472. Fires on **x86_64 only**: aarch64
  always produced the canonical value, so its sealed bytes are identical with and without the fix. A
  green aarch64 leg is not evidence that the pin is still in place.

Both were verified to actually fail with the `[patch.crates-io]` block removed, rather than assumed to.

