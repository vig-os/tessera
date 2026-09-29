---
type: issue
state: closed
created: 2026-07-03T22:20:27Z
updated: 2026-09-28T17:55:04Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/337
comments: 2
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-09-29T07:52:48.140Z
---

# [Issue 337]: [chore(release): cut 0.1.0-alpha.1 — dispatch release-plz + publish credentials](https://github.com/vig-os/tessera/issues/337)

Tracking issue for the **operator steps** to actually cut the first tagged release, `0.1.0-alpha.1`. Follows ADR-0052 and the C→A landing (#336). Step 1 (bootstrap the workspace version) is done separately; everything below is external/credential-gated and needs the repo owner.

## Context
The pipeline is set up and **held** (ADR-0052):
- `release-plz.yml` trigger = `workflow_dispatch` (manual).
- `release-plz release-pr` opens a HELD version-bump + CHANGELOG PR; nothing is tagged/published until that PR is deliberately merged.
- The sealed `producer` is decoupled from the crate version (#336), so version bumps cost **no corpus regen**.
- The workspace version has been bootstrapped `0.0.0` → `0.1.0-alpha.1` (regen-free; conformance corpus unchanged).

## Steps to cut the alpha (owner)
- [ ] **Enable PR creation by Actions** — Settings → Actions → General → "Allow GitHub Actions to create and approve pull requests". *Or* add a PAT as `secrets.RELEASE_PLZ_TOKEN` (used first when present). Without one of these, `release-plz release-pr` cannot open the PR.
- [ ] **Dispatch `release-plz`** — Actions → release-plz → "Run workflow" on `dev`. It opens/updates the held release PR (version + `CHANGELOG.md`).
- [ ] **Merge the release PR** → this is the deliberate "cut" surface. A later, gated `release-plz release` step tags + publishes.
- [ ] **Flip `main`** to the tagged release (`dev` → `main`), per the `dev`=moving-target / `main`=distribution model.
- [ ] **After the first release:** switch the `release-plz.yml` trigger back to `push: [dev]` so the release PR auto-updates on every merge.

## Publishing credentials (separate, when ready to publish artifacts)
- [ ] `CARGO_REGISTRY_TOKEN` (crates.io) — set `publish = false` on `tessera-py` / `tessera-wasm` (cdylibs → PyPI/npm, not crates.io) in `release-plz.toml` before the first publish.
- [ ] **PyPI wheel** via maturin (`tessera-py`, abi3).
- [ ] **Prebuilt binaries** via cargo-dist (`tessera` / `tsra` CLI).

## Release-notes must-say
`0.1.0-alpha.1` is a **software** alpha. The **format** version (`tessera_version`) stays `v0` / pre-1.0 and is deliberately unfrozen — a software alpha does not imply a stable format (ADR-0052 §1).

## References
ADR-0052 · #336 (producer decouple / C→A) · #331 (pipeline set-up) · `release-plz.toml` · `.github/workflows/release-plz.yml`
---

# [Comment #1]() by [gerchowl]()

_Posted on September 23, 2026 at 11:52 PM_

**v0.1.0-alpha.1 is published** — https://github.com/vig-os/tessera/releases/tag/v0.1.0-alpha.1 (prerelease; 13 assets: Linux + macOS x86_64/aarch64 tarballs + checksums, `tessera-cli-installer.sh`, source).

Operator steps done: PR creation by Actions solved with a GitHub App (#430/#432) · release-plz dispatched → #435 merged (the cut) · tag `v0.1.0-alpha.1` on `a35c61b` · `main` flipped to the released commit · release-plz switched to `push: [dev]` (#436). Two release-pipeline fixes were needed on the way: cargo-dist regenerates `release.yml` (#429, SHA pins #434) and macOS died on `brew bundle exec` (#437 dropped the redundant Homebrew cmake dep). `release-nix` (nix channel + abi3 wheel) is running on the publish event.

Held by design: crates.io (`CARGO_REGISTRY_TOKEN`) and PyPI — no registry publish is wired; consumers use the GitHub Release, the nix channel, or git/flake refs.

---

# [Comment #2]() by [gerchowl]()

_Posted on September 28, 2026 at 05:55 PM_

Closing as **done** — verified on `origin/dev` in the 2026-09-28 backlog triage.

Evidence: 0.1.0-alpha.1 tagged + published 2026-09-24 (memory + commit a3f4a99 'chore: release v0.1.0-alpha.1 (#435)'; tag a35c61b). Remaining publish lines (PyPI wheel, crates.io) move to #440 / #441 (devkit release-train adoption, vig-os/devkit#1746).

https://claude.ai/code/session_01XdERKMVDAwfMJSKdTytNnK

