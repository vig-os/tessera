---
type: issue
state: open
created: 2026-10-18T12:15:04Z
updated: 2026-10-18T12:15:04Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/607
comments: 0
labels: chore, priority:low, area:ci
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-19T07:17:50.163Z
---

# [Issue 607]: [ci(release): enable cargo-semver-checks in the release PR (needs libhdf5 where release-plz runs)](https://github.com/vig-os/tessera/issues/607)

## Summary

With #606 the release-plz workflow compares against the last release tag, which makes release-plz run
cargo-semver-checks on the library crates (`tessera-core`, `tessera-io`, `tessera-ingest`). #606 sets
`semver_check = false` in `release-plz.toml`, because:

- cargo-semver-checks builds rustdoc for the current AND the released tree, and `tessera-ingest` links
  the system libhdf5 (`hdf5-metno-sys` via pkg-config). The bare `ubuntu-latest` runner has no libhdf5,
  so the check cannot build there.
- On a pre-release line its verdict cannot change the bump (`0.1.0-alpha.N` → `alpha.N+1`); it only
  annotates the release PR.

Before this, the check never ran either: the registry baseline had no packages to compare against.

## Proposal

Before a stable (non-pre-release) version, run `release-plz release-pr` where libhdf5 resolves (the nix
dev shell, `nix develop --command release-plz …`, with `cargo-semver-checks` added to the shell), and
then set `semver_check = true`. Alternatives: the `static-hdf5` feature for the rustdoc build, or
`apt install libhdf5-dev` on the runner (drifts from the nix-pinned HDF5).

## Acceptance

- The release PR body carries a semver-checks verdict for the three library crates.
- `semver_check` is removed from `release-plz.toml` (default on), with the reason comment updated.

