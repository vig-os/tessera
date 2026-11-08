---
type: issue
state: open
created: 2026-11-08T03:03:43Z
updated: 2026-11-08T04:35:27Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/772
comments: 1
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-08T08:33:50.985Z
---

# [Issue 772]: [ci: run every eligible CI lane on the ksb ARC runners](https://github.com/vig-os/tessera/issues/772)

Move every CI lane that can run on the ksb ARC (k3s) runners onto them; #688 did this only for nix-check and the macOS-build guard. Labels: `ksb-vig-os-{small,large,xlarge}` (small 2 CPU/8Gi, large 8/32Gi, xlarge 16/32Gi).

Still on GitHub-hosted runners on dev:
- the `ci.yml` devkit lanes (`DEVKIT_CI_RUNNER=` empty in `.vig-os`; routed via `resolve-toolchain`'s `runner-json`);
- `ci.yml` jobs on `ubuntu-26.04`;
- `codeql.yml`, `scorecard.yml`, `sync-issues.yml`, `devkit-upgrade.yml`, `release-plz.yml`;
- `release.yml` (cargo-dist generated) and `release-nix.yml`;
- the `nix-check` aarch64 leg (`ubuntu-24.04-arm`).

**Acceptance**
- Each workflow is moved, or kept hosted with a documented reason. Expected exceptions are aarch64 and macOS (no ksb hardware) and any action that refuses self-hosted runners (verify, don't assume; e.g. Scorecard `publish_results`).
- Scaffolded files (`ci.yml`, `codeql.yml`) move **only** through `.vig-os` knobs (`DEVKIT_CI_RUNNER`, `DEVKIT_CI_RESOLVE_RUNNER`, the ephemeral-runner gcroot path, #1601). Never hand-edit them; `scaffold-drift` stays green. cargo-dist's `release.yml` moves only via `[dist]` config.
- **Fork guard on every ksb lane** (public repo): fork PRs fall back to hosted or don't run, the same pattern as nix-check.yml.
- Runner sizing is measured, not guessed; record each lane's peak memory on its first ksb run.
- Known ksb traps (memory: ksb-arc-nix-gotchas): job `env: NIX_CONFIG` replaces the pod config; `available_parallelism()` reports 84 node CPUs → budget from `$NIX_BUILD_CORES`; pod-max-pids 4096.
- Owner note: the ksb node's disk is overloaded (exo-fleet#980), so wall-time changes are recorded as context, not as a regression or a win.
- Every moved lane runs green on a PR. Release lanes are verified by a `workflow_dispatch` dry run where one exists; never cut a tag.

Depends on #771 (devkit 1.19.0 adoption) (both touch `.vig-os` and `ci.yml`).


---

# [Comment #1]() by [gerchowl]()

_Posted on November 8, 2026 at 04:35 AM_

## Diagnosis: why the ci.yml lanes are cold on ksb (PR #774, run 42443603731)

**Root cause: ksb infra, not tessera or devkit configuration.** Filed as exo-pet/exo-fleet#1102.

- **Refused cache server.** `http://10.0.2.2:8093` is ksb's nginx pull-through mirror of cache.nixos.org (`ksb-nix-cache-proxy.service`, exo-fleet#1089). Since it entered the pod config (it was absent in 2026-11-06 pods), every tessera pod has logged `Failed to connect … after 0 ms` and `disabling binary cache`, on large and small pods alike. It still refuses from the seat now, while `:8092/ksb` (attic) answers.
- **Why the pods are cold.** Every ARC pod starts from a seeded per-pod store, and attic keeps no paths that cache.nixos.org already has. With the proxy down, each pod pulls the dev shell from cache.nixos.org: 160 of 176 paths on the Tests pod before the cancel, 16 from attic. The first two of those paths are the two nixpkgs flake-input sources, 35 MB and 50 MB compressed (206 and 215 MB unpacked).
- **Slow pods today.** The same `pfwrb65…-source` took **7 s** on a large pod on 11-07, but **5 to 7.5 min** on each of today's three small pods. From the seat, the NAR downloads in **0.34 s**, so the uplink is fine and the compute VM's disk or the pod net path is the suspect (exo-fleet#980). Those wall times are context only, per the owner note.

**Knobs checked and ruled out**

- **`NIX_CONFIG`:** correct. devkit 1.19's `Configure host Nix` keeps the pod config (substituters `8093 8092/ksb cache.nixos.org`, `cores = 4`, `sandbox = false`) and appends `vig-os.cachix.org`. Verified in the step env of all three jobs.
- **`DEVKIT_DEV_PROFILE_PATH` (#1601):** not applicable on ksb. That knob protects a `/nix/store` that persists while `RUNNER_TEMP` is wiped, but an ARC pod's whole store dies with the pod. No persistent path exists to point it at, so setting it would root nothing.
- **What `nix-check.yml` does differently:** it never realises the dev shell. Its deps come from the GitHub Actions cache through `nix-deps-cache` (795 of 1130 paths `file://` on 11-07). It also runs on `large` with a 120-min cap. It hits the same 8093 refusal, but its time budget absorbs it.
- **Sizing (`DEVKIT_CI_LANE_SIZES`):** not measurable yet. No lane got past `Set up devkit toolchain`, so there is no realisation time or memory peak to size from. The hosted baselines are toolchain ~1 min and pre-commit ~4 min (run 42441529958). Widening timeouts is rejected per the brief.

**Side effect of #774 to know before merging it.** `sync-issues.yml` (sync job, 10-min cap, runs `setup-devkit-toolchain`) and `devkit-upgrade.yml` (30-min cap, `nix develop -c git commit`) also resolve to `vars.CI_RUNNER_SMALL`. Once #774 is on dev, the nightly sync and the weekly upgrade move to ksb small too, onto the same cold path. Before #774, all their runs were hosted.

**Remaining lanes (step 3, not started; preliminary notes)**

- `codeql.yml`: hard-coded `ubuntu-26.04`, and no `.vig-os` knob routes it. Moving it needs a devkit knob.
- `scorecard.yml`: `publish_results: true`. Whether it refuses self-hosted runners is still to be verified.
- `release-plz.yml`: stays hosted per #688, because it mints the `RP_APP_*` token with `contents: write`.
- `release-nix.yml`: x86_64 on `ubuntu-latest`, aarch64 has no ksb hardware.
- `release.yml`: moves only through `[dist]` config.

Step 3 waits on the step-1 blocker, because every Nix lane moved now would land on the same cold path.

**Next once exo-fleet#1102 is fixed:** re-run the ci.yml lanes on #774, record realisation time and peak memory per lane, set `DEVKIT_CI_LANE_SIZES` from the numbers, then step 3.


