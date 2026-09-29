---
type: issue
state: closed
created: 2026-08-20T09:01:10Z
updated: 2026-09-28T17:55:26Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/402
comments: 2
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-09-29T07:52:39.344Z
---

# [Issue 402]: [ci: drop `static-hdf5` from the PR feature matrix — it dominates the ~90 min x86_64 flake check](https://github.com/vig-os/tessera/issues/402)

## Motivation

The x86_64 `nix flake check` runs ~90 min, and the dominant leg is `static-hdf5` **building HDF5
2.2.0 from source** inside the `--all-features` clippy gate (`flake.nix:249`,
`cargoClippyExtraArgs = "--all-targets --all-features -- -D warnings"`).

That cost is paid on **every PR**, for a feature that only changes **how libhdf5 links** — not what
any code does. `tessera-ingest/Cargo.toml` says so itself:

> *"SAFE for determinism: HDF5 is read-only input (Tessera encodes to Vortex/pcodec), so the libhdf5
> version is never in the sealed byte-path — it cannot move any content_hash."*

So the PR gate is spending most of its wall-clock compiling a C library whose only job is to be
linked differently, and which cannot affect correctness of the thing under review.

This surfaced while spiking #398. ADR-0056 §12 cites CI cost as one of three reasons to feature-gate
ingest decoders, and the #398 review panel independently concluded that **the CI-cost argument is not
actually fixed by gating decoders — it is fixed here**, by moving `static-hdf5` out of the PR matrix.
Separating the two means the packaging decision (ADR-0057) is not carrying a CI problem it cannot
solve.

## Decision / proposed approach

Make `static-hdf5` **release-only**:

- PR gate: clippy/tests over the feature set **minus** `static-hdf5` — libhdf5 comes from the nix
  closure (`buildInputs = [ hdf5 ]`), which is already how the devShell works.
- Release channels keep it ON, unchanged: cargo-dist (`/dist-workspace.toml`,
  `features = ["static-hdf5"]`) and the maturin wheel. That is where a self-contained binary matters.
- Keep **one** scheduled or release-gated job that does build with `static-hdf5`, so the vendored
  build never silently rots between releases.

Note this interacts with ADR-0057 §4, which folds `static-hdf5` into the `hdf5` capability feature
(`static-hdf5 = ["hdf5", "hdf5-metno/static", "hdf5-metno/zlib"]`) so it cannot be set without the
decoder it configures. Either ordering works; if ADR-0057 lands first, the exclusion is expressed
against the new feature graph.

## What already exists

- `flake.nix:60` — a comment already identifying this exact cost:
  *"any check that enables all features — e.g. the `--all-features` clippy — compiles that"*
- `flake.nix:63-64` — `nativeBuildInputs = [ clang pkg-config cmake ]`, `buildInputs = [ hdf5 ]`, so
  a system libhdf5 is already available to non-static builds
- `tessera/crates/tessera-ingest/Cargo.toml` — the `static-hdf5` feature and its determinism rationale
- `/dist-workspace.toml` — the release channel that legitimately needs it, plus its
  `[dist.dependencies.apt] cmake` / `[dist.dependencies.homebrew] cmake` provisioning

## Scope

**P0**
- [ ] PR clippy/test matrix excludes `static-hdf5` (explicit feature list rather than bare
      `--all-features`, or an equivalent exclusion)
- [ ] measure and record the before/after wall-clock of the x86_64 flake check

**P1**
- [ ] a release-gated (or scheduled) job that still builds with `static-hdf5` on both arches, so the
      vendored HDF5 build cannot rot unnoticed
- [ ] comment in `flake.nix` stating *why* the exclusion exists, so a future "let's just use
      --all-features" does not silently undo it

## Pitfalls

- **`--all-features` is a blunt instrument, and replacing it with an explicit list is exactly how
  features stop being linted.** Whatever replaces it must be derived or asserted, not hand-copied —
  a feature added later must not silently escape clippy.
- Dropping the static build from PRs means a PR can break the vendored HDF5 build and go green. The
  P1 release-gated job is therefore **not optional**, it is the other half of this change.
- `hdf5-metno/static` also pulls `zlib`; confirm the non-static path still has gzip/deflate filter
  support from the nix `hdf5` (it should — nixpkgs builds HDF5 with zlib).
- The aarch64 leg has different timings; measure both before declaring a win.
- Do not conflate this with feature-gating the HDF5 *decoder* (ADR-0057 §5). This issue changes only
  **how libhdf5 links in CI**, not whether the decoder compiles.

## Acceptance criteria

- [ ] x86_64 `nix flake check` wall-clock measurably reduced, number recorded in the PR
- [ ] Every Cargo feature is still linted by *some* PR job
- [ ] A deliberately-broken vendored HDF5 build is still caught before a release is cut
- [ ] Release artifacts are byte-for-byte as self-contained as they are today

## References

- ADR-0057 §4 (`docs/adr/0057-ingest-topology-thin-waist-and-release-matrix.md`) — the `static-hdf5`
  → `hdf5` feature relationship, and why forking the shipped binaries does **not** fix CI cost
- ADR-0056 §12 — cites CI cost as a motivation for gating decoders
- #374 / #375 (cargo-dist + static-hdf5), #376 / #377 (nix channel), #398 (spike)

Refs: #398
---

# [Comment #1]() by [gerchowl]()

_Posted on August 20, 2026 at 06:57 PM_

**P0 landed in #404** (ADR-0057 Phase 0), commit `build(ci): drop static-hdf5 from the PR clippy matrix`.

- [x] PR clippy/test matrix excludes `static-hdf5` — `flake.nix` now builds an explicit
      `workspaceFeatures` set (`tessera-core/full · tessera-io/cloud · tessera-cli/cloud ·
      tessera-cli/sql`) instead of a bare `--all-features`.
- [~] measure and record the before/after wall-clock — the **before** is recorded in the PR from the
      two most recent full CI runs on `dev` (x86_64 **63m49s** / **46m40s**; aarch64 49m14s /
      46m19s). The **after** is this PR's own CI run, on both arches, so the number lands on the PR
      rather than here.

On the pitfalls this issue raised:

- *"replacing `--all-features` with an explicit list is exactly how features stop being linted"* —
  taken seriously. The exclusion is **measured, not assumed**: a full package diff of the two
  resolved graphs shows exactly one package difference, `hdf5-metno-src v0.10.3` (the vendored HDF5
  CMake build). Nothing else leaves the lint. The list is commented in `flake.nix` with the rule that
  a new feature must be added to it, and Gate B (#400, same PR) snapshots the graph under
  `--all-features`, so the `static-hdf5` corner of the feature space stays under structural watch
  even though clippy no longer builds it.
- *"confirm the non-static path still has gzip/deflate filter support"* — unchanged by this PR: the
  nix `hdf5` from `buildInputs` is what every non-static build already linked, and every HDF5 test in
  the suite passes in `nix flake check`.
- *"do not conflate this with feature-gating the HDF5 decoder"* — not conflated. ADR-0057 §5's
  feature graph is Phase 1; this changes only how libhdf5 links in CI.

**P1 remains open and is the other half of this change.** A PR can now break the vendored HDF5 build
and go green: `dist` only *plans* on pull requests and only *builds* on a release-plz tag, so nothing
in the PR path compiles `static-hdf5` any more. The scheduled/release-gated static build on both
arches is the missing guard.

---

# [Comment #2]() by [gerchowl]()

_Posted on September 28, 2026 at 05:55 PM_

Closing as **done** — verified on `origin/dev` in the 2026-09-28 backlog triage.

Evidence: commit 2b7821c 'drop static-hdf5 from the PR clippy matrix (ADR-0057 §4)' — flake.nix workspaceFeatures now excludes static-hdf5.

https://claude.ai/code/session_01XdERKMVDAwfMJSKdTytNnK

