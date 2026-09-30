---
type: issue
state: open
created: 2026-09-29T10:22:26Z
updated: 2026-09-29T10:22:26Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/506
comments: 0
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-09-30T07:57:09.817Z
---

# [Issue 506]: [ci(nix): scope the seal-profile-determinism dev half by paths, not by branch — and fix the gate hole in the comment's own remedy](https://github.com/vig-os/tessera/issues/506)

## Context

`nix flake check` legs are memory-bound (#495): #497 was OOM-killed (exit 137) at 15.5 GB on a
16 GB runner. `flake.nix:377-384` already documents one cause and names its own remedy:

> `cargoArtifacts` (`buildDepsOnly`) pre-builds the **release** dependency graph, so the release half
> of this check is warm while the dev half compiles vortex and friends from source on every run. That
> is the bulk of this leg's time. […] the cheaper move is to run it release-only on PRs and keep both
> profiles for `main`.

That remedy as written is **not safe**, and the measurement also shows its benefit is not what the
comment claims. Both corrections below.

## Correction 1 — "release-only on PRs, both profiles on main" opens a gate hole

`nix-check.yml` triggers on `pull_request` (any base) and `push: main` — **not** on pushes to `dev`.
So dropping the dev-profile half from PRs means a #468-class debug/release divergence would not
surface on the PR that caused it, nor when it merged to `dev`, but only at the eventual release PR
into `main` — with every intervening commit to bisect. The whole value of the gate is catching it at
the cause.

## Correction 2 — the benefit is PEAK MEMORY, not wall-clock

Measured on #500's successful x86 leg (run 36543170354):

| derivation | span | duration |
|---|---|---|
| `tessera-nextest` | 08:56:34 → 09:55:05 | **58.5 min** |
| `tessera-seal-profile-determinism` | 09:15:57 → 09:47:54 | 32.0 min |

The leg ended at 09:55:28 — **23 seconds after `nextest` finished**. `nextest` is the critical path;
`seal-profile-determinism` ends ~7 minutes before the leg does. Removing it saves **≈0 wall-clock**.

What it does remove is a **32-minute heavy compile running concurrently** with `nextest`'s own heavy
window, which is exactly the overlap that produced #497's `+8.5 GB in 4 minutes` spike. So this is a
peak-memory fix and should be justified on that basis only.

## Proposal: a paths filter, not release-only

Keep the dev-profile half on PRs **that touch the encoder dependency surface**, skip it otherwise:

```
tessera/Cargo.lock · tessera/crates/tessera-io/** · tessera/crates/tessera-core/**
flake.nix · **/Cargo.toml   (the vortex pin lives in tessera/Cargo.toml's [patch.crates-io])
```

This preserves catch-at-cause for every change that *can* cause a profile divergence, and removes
the compile for those that cannot. Checked against the two branches that drove #495:

| PR | touches the surface? | effect |
|---|---|---|
| **#497** (bench/CLI only — the branch that was **OOM-killed**) | **no** (0 matching paths) | dev half **skipped**, 32-min concurrent compile removed from the leg that died |
| #461 (npy/npz; touches `Cargo.lock`, `tessera-ingest/Cargo.toml`, `tessera-io/src/container.rs`) | yes (2) | gate **kept** — correctly, it could cause divergence |

## Rejected alternative: a second dev-profile `buildDepsOnly`

Evaluated per the suggestion. **There is no nix store cache in this workflow** — only
`nix-installer-action`; no `magic-nix-cache`, FlakeHub Cache or cachix. The #497 log shows 739 paths
substituted from `cache.nixos.org` and everything project-specific built from source. So a second
`buildDepsOnly` would be rebuilt on **every run** with nothing to amortise it, adding a full
dev-profile dependency compile rather than removing one. Crane also gives a derivation exactly one
`cargoArtifacts`, so `seal-profile-determinism` would have to split into two derivations plus a diff.
Complexity up, compile volume roughly unchanged, no caching to pay it back.

## The larger prize, noted separately

The absence of a nix store cache is itself the biggest available win: with a warm store, unchanged
derivations are *substituted rather than compiled*, which cuts wall-clock and peak memory together —
the only lever besides a bigger runner that improves both. Worth its own issue; flagged here because
it is what makes the rejected alternative unattractive.

Refs: #495

