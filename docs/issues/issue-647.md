---
type: issue
state: closed
created: 2026-10-19T05:32:37Z
updated: 2026-10-19T11:09:52Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/647
comments: 2
labels: bug
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-20T07:08:35.924Z
---

# [Issue 647]: [fix(release): macOS dist builds fail — posix_fadvise in tessera-cli + tessera-py dylib link; no PR-time macOS gate](https://github.com/vig-os/tessera/issues/647)

## What happened

The tag push for `v0.1.0-alpha.2` (075cfea) ran the cargo-dist Release workflow ([run 39203946447](https://github.com/vig-os/tessera/actions/runs/39203946447)). Both Linux builds succeeded. **Both macOS builds failed**, so `announce` was skipped and no GitHub Release was created. alpha.1 built on both macOS targets.

There are two independent defects.

### 1. `tessera-cli` calls a Linux-only libc API

```
error[E0425]: cannot find function `posix_fadvise` in crate `libc`
error[E0425]: cannot find value `POSIX_FADV_DONTNEED` in crate `libc`
error[E0308]: mismatched types
error: could not compile `tessera-cli` (bin "tessera") due to 3 previous errors
```

The call is at `crates/tessera-cli/src/bench_compare.rs:214-234`. It was introduced by #487 (41b0801, `tessera bench compare`). Darwin has no `posix_fadvise`. Its cache-eviction primitive is `fcntl(F_NOCACHE)` or `purge`, with different semantics. The cold-cache mode needs a decided per-platform behaviour, for example an honest "cold unsupported on this OS" error or a documented macOS method. A silent no-op that labels warm reads "cold" is not acceptable; see the bench fairness rules in `bench/ecosystems/README.md`.

### 2. The `tessera-py` `_native` dylib fails to link on macOS

```
Undefined symbols for architecture arm64:
  "_PyBaseObject_Type", referenced from: ... Builder::__pymethod___new__
  "_PyBool_Type", "_PyByteArray_AsString", "_PyBytes_AsString", ...
error: linking with `cc` failed: exit status: 1
```

The Python symbols are unresolved because the dylib is linked without `-undefined dynamic_lookup`. pyo3 is still 0.29.0 with `extension-module` (unchanged since alpha.1), so the regression comes from something else that changed since alpha.1. Candidates: dist building `tessera-py` at all, a workspace or dist-config change, or a dependency that changed link args. Root-cause it; don't guess.

## Root cause in CI

Neither PR CI nor `nix flake check` compiles for any Darwin target, so macOS breakage only surfaces at tag time, after the release-plz PR has merged. The fix needs a PR-time macOS build gate.

## Acceptance

- [ ] `cargo build --release` for `tessera-cli` and `tessera-py` succeeds on aarch64-apple-darwin and x86_64-apple-darwin, proven on macOS CI runners.
- [ ] The cold-cache behaviour on macOS is decided and documented, with a test.
- [ ] A PR-time CI job builds the dist targets for macOS, so this class fails before merge. It must be shown to fail on current dev.
- [ ] `v0.1.0-alpha.2` or its successor publishes with all cargo-dist assets.

---

# [Comment #1]() by [gerchowl]()

_Posted on October 19, 2026 at 09:23 AM_

Release decision (owner, 2026-10-19): the `v0.1.0-alpha.2` tag (075cfea) stays as a **failed cut**. Pushed tags are immutable, and no GitHub Release was ever created for it. Once the fix (#648) merges, release-plz proposes `0.1.0-alpha.3`, which ships the alpha.2 changes plus this fix. Install alpha.3, not the alpha.2 tag.

---

# [Comment #2]() by [gerchowl]()

_Posted on October 19, 2026 at 11:09 AM_

Fixed by #648 (aa99b87). Its last acceptance item is now met: **v0.1.0-alpha.3 is published** (https://github.com/vig-os/tessera/releases/tag/v0.1.0-alpha.3). Release run 39239676788 built all four targets, including both darwin legs, and produced 13 assets. Smoke test: the x86_64-linux archive checksum is OK, `tessera --version` reports 0.1.0-alpha.3, and `verify` passes on a corpus product. Follow-ups: #650 (MACOSX_DEPLOYMENT_TARGET), #651 (review nits), #653 (changelog compare links).

