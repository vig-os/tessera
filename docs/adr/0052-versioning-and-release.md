# ADR-0052 — Versioning policy & release pipeline (release-plz)

**Status:** Accepted (2026-07-03, as-set-up). Process/governance decision (not a feature row → exempt
from the adr-matrix gate). Relates the Tessera→`dev` graduation (PR #322) and the `dev`=moving-target /
`main`=distribution branch model.

## Context

At the fd5→Tessera graduation, all crates are `0.0.0`, there is no versioning policy, and the only
release infra is a Python-template `release.yml` that doesn't understand the Cargo workspace. Before any
tag we need: (1) a versioning policy, and (2) a pipeline.

## Decision

### 1. Two independent version axes — do not conflate

- **Software version** (the crates / CLI / bindings) — **SemVer**, starting **`0.1.0-alpha.1`**.
- **Format-spec version** (`tessera_version` in a `.tsra`) — stays **`v0` / pre-1.0**, deliberately
  *unfrozen* (SPEC self-designates pre-1.0). A software alpha release does **not** imply a stable
  format; release notes state this explicitly.

These evolve separately: the software can reach `0.3.0` while the format is still `v0`; freezing the
format to `v1` is its own decision (RFC §14), independent of software SemVer.

### 2. Unified workspace version

All crates share one `[workspace.package] version`, bumped in lockstep. Simpler than per-crate for an
alpha; `tessera-core`/`io`/`cli`/`ingest`/`py`/`wasm` release together. (`tessera-py`/`tessera-wasm` are
cdylibs → PyPI/npm, not crates.io; marked `publish = false` for crates.io when publishing is wired.)

### 3. Pipeline = release-plz (Rust-native)

**[release-plz](https://release-plz.dev)** over release-please — it understands the Cargo workspace
natively (crate versions, inter-crate deps, `cargo publish`), drives versioning + CHANGELOG from
**conventional commits** (already the repo's commit style), and its release flow is a **release PR** that
sits until merged. Chosen over release-please (generic + a rust plugin + a bolted-on publish step).

- **`release-plz release-pr`** runs on pushes to `dev`: opens/updates a PR bumping the workspace version
  + regenerating `CHANGELOG.md`. **Nothing is tagged or published by this.**
- **The release is *held* by construction** — `0.1.0-alpha.1` only happens when that release PR is
  **deliberately merged**, then `release-plz release` (a later, gated step) tags + publishes.
- **`release = false`** in `release-plz.toml` for now → no auto-publish. crates.io publishing is wired
  when a `CARGO_REGISTRY_TOKEN` is provided (external / owner's call). PyPI (maturin) + binaries
  (cargo-dist) are complementary, added when the alpha is actually cut.

### 4. Flow

```text
feature → PR → dev            (moving target; CI per change)
push to dev → release-plz opens/updates a HELD release PR (version + CHANGELOG)
merge the release PR → tag 0.1.0-alpha.1 → (later, with token) cargo publish
dev → main                    (distribution: main flips to the tagged release)
```

## Consequences

- No hand-tags; the version history is release-plz-owned + conventional-commit-driven.
- The first alpha is *ready but held* — the release PR is the "cut when you're ready" surface.
- The Python-template `release.yml` is superseded by `release-plz.yml` (kept or removed separately).
- Registry publishes stay off until credentials + `publish`-scoping are set — a deliberate, separate step.

## Update (2026-07-03) — producer decoupled from the crate version; trigger held

The first release-plz runs surfaced that the sealed `Manifest.producer` embedded `CARGO_PKG_VERSION`
(`product.rs`), so *any* crate version bump changed every `manifest_hash` → a full conformance-corpus
regen per release. **Fixed (C):** the sealed `producer` now stamps the **format** version
(`TESSERA_VERSION`), not the crate version — byte-identical today (both `0.0.0` → **no regen**), but a
future crate bump no longer touches the seal. The build/software version stays in the non-sealed
`aux/provenance.json` (ADR-0042). Software SemVer and format identity are now truly independent.

**Release held (A):** the workflow trigger is `workflow_dispatch` (manual) while the crates are `0.0.0`
with no baseline (release-plz can't determine a next version pre-first-release). To cut `0.1.0-alpha.1`:
bootstrap the workspace version (now regen-free), then dispatch release-plz → it opens the held release
PR. Switch the trigger back to `push: [dev]` after the first release for auto-updating PRs.

## References

RFC §14 (semver policy / format supersession) · PR #322 (Tessera→dev graduation) · `release-plz.toml`
· `.github/workflows/release-plz.yml` · the `dev`=moving-target / `main`=distribution branch model.
