---
type: issue
state: open
created: 2026-09-29T10:24:46Z
updated: 2026-10-05T07:38:47Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/507
comments: 1
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-05T08:17:46.066Z
---

# [Issue 507]: [ci(nix): no nix store cache — every run recompiles the dependency graph from source (~19.4 min, 23% of each leg, measured)](https://github.com/vig-os/tessera/issues/507)

## The gap

`nix-check.yml` installs Nix (`DeterminateSystems/nix-installer-action`) and nothing else — there is
**no nix store cache**: no magic-nix-cache, no GitHub Actions cache of `/nix/store`, no cachix, no
FlakeHub Cache. Every run starts with a cold local store: the #497 log shows 739 paths substituted
from `cache.nixos.org` (public deps) and **everything project-specific compiled from source, on every
run, on both arches**.

## The measured cost

Per-derivation spans from #500's successful x86 leg (run 36543170354, `--cores 2`, 84.8 min):

| derivation | duration |
|---|---|
| `tessera-deps` (`buildDepsOnly`) | **19.4 min** |
| `tessera-core-wasm-deps` | 0.6 min |
| `tessera-wasm-bindings-deps` | 0.7 min |
| **total dependency compile** | **20.7 min** |

`tessera-deps` is a **serial prefix**, not one cost among many: the first heavy check
(`tessera-cli`/`tessera-cli-cloud`/`tessera-clippy`) starts at 08:53:07, two seconds after it
finishes, i.e. **22.4 minutes into the leg nothing else has started**. Its inputs are keyed on
`Cargo.lock`, which is unchanged on most PRs.

**A warm store therefore removes ~19.4 min from an 84.8 min leg — 23%, per arch, per run.** That is
larger than any knob discussed on #495, and unlike them it costs nothing in memory or coverage.

## What it does NOT fix — stated because the opposite is the intuitive reading

It is **not** a memory fix. Measured on the same leg:

```
deps window (08:33-08:53):  peak  4633 MB   mean 3168 MB
after deps  (08:53-09:55):  peak 11957 MB   mean 5945 MB
```

The dependency build is the *cheapest* phase for memory; the peak comes from the concurrent check
derivations that run after it. Caching deps leaves that peak untouched. A warm store lowers peak only
when whole *check* derivations are substituted — which happens on PRs that do not touch the Rust
source, and those are the PRs that least need it. **The OOM work (#495, #506) is unaffected and still
needed.** This issue is a wall-clock fix and should be judged as one.

## Options, split by who can decide them

**Repo-owned** (`nix-check.yml` is this repo's, not devkit-managed):

- **GitHub Actions cache of the store** — `actions/cache` over `/nix/store` plus a db restore, or
  `DeterminateSystems/magic-nix-cache-action`. No external account, no secrets. Constraints to check
  before committing: the 10 GB per-repo Actions cache limit (our store is large — needs measuring),
  and that magic-nix-cache is sunset in favour of FlakeHub Cache, so it may not be a durable choice.

**Owner call** (external account + secrets):

- **Cachix** or **FlakeHub Cache** — larger and purpose-built, but both need an account and a signing
  key in secrets, which is a decision for the repo owner and not one I should make.

## devkit interaction — check before implementing

devkit owns the CI scaffold, and a substituter is **not** purely a workflow concern: devkit bakes
`nix.conf`, and devkit#773 (*"Harden baked nix.conf: drop accept-flake-config=true for explicit
substituters + trusted-public-keys"*) established explicit substituters + trusted public keys as the
policy. Any cachix/FlakeHub substituter has to go through that mechanism rather than around it.

A search of vig-os/devkit for `nix cache` / `magic-nix-cache` / `cachix` / `flakehub` returns **no
open issue proposing a store cache** — only #773 (closed, the hardening above) and #1746 (closed,
unrelated release train). So this appears unowned upstream; if it is wanted fleet-wide it likely
belongs as a devkit issue with this measurement attached, rather than only here.

## Suggested order

1. Measure the actual store size on a CI leg (decides whether the free Actions cache is even viable).
2. If viable, do the repo-owned option — it needs no owner decision and captures the 23%.
3. Raise the substituter-based options with the owner and with devkit only if (1) rules the free path out.

Refs: #495, #506

---

# [Comment #1]() by [gerchowl]()

_Posted on October 5, 2026 at 07:38 AM_

**Watch item: cache budget** (from the #544/#548 review). Repo Actions cache is at **6.92 GB of 10 GB** (33 entries). Five of them are `nix-deps-v1` entries at 1.09–1.15 GB each, all branch-scoped by-products of this work: `perf/507`, `perf/517` and `pull/544`. The rest are ~0.05 GB nix-installer entries.

Steady state after #544 merges:
- **dev scope:** one entry per arch, 2.24 GB in total. It's refreshed only when the deps derivation changes, and touched by every PR's restore, so LRU eviction keeps it.
- **PR scope:** only PRs that **change** the deps derivation save their own entry (~2.2 GB for both arches). A hit never saves. The obvious source is Renovate's Cargo.lock bumps: about 3–4 of them open at once would push the cache past 10 GB, and GitHub then evicts least-recently-used entries.
- **Evicting the branch entries above:** harmless. They're unused 7 days after these PRs land and age out.

No change for now. If the cap bites, skip the PR-scope save on `pull_request` runs, so only the warmer writes. Re-runs of a deps-changing PR would then go cold.

