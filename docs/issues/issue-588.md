---
type: issue
state: open
created: 2026-10-17T08:10:40Z
updated: 2026-10-17T08:10:40Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/588
comments: 0
labels: bug
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-10-18T06:56:50.490Z
---

# [Issue 588]: [deps: RUSTSEC advisories in rustls 0.23.45 and zip 2.4.2 fail cargo-deny on every PR](https://github.com/vig-os/tessera/issues/588)

## The defect

Since about 2026-10-16, `cargo-deny` (Lint & Format → CI Summary) fails on every PR (first seen on #585, unrelated to its diff):

| advisories | crate | locked | fixed in |
|---|---|---|---|
| RUSTSEC-2026-0346/0347/0348/0349 | rustls | 0.23.45 | ≥ 0.23.46 |
| RUSTSEC-2026-0367/0368/0369 | zip | 2.4.2 | ≥ 9.0.0 |

## The fix: no `ignore` by reflex

- **rustls**: `cargo update -p rustls` to ≥ 0.23.46 (a semver-compatible lockfile bump).
- **zip**: the advisories are in `ZipArchive::extract` / `ZipStreamReader::extract` (symlinks, SUID bits). Tessera calls
  `extract(` nowhere (`grep -rn '\.extract(' crates/*/src` → 0), so it's not reachable today. But 2.4 → 9 is a major upgrade of
  the crate that WRITES the sealed `.tsra` container (workspace dep, `default-features = false`, STORED zip64; tessera-ingest
  enables deflate-flate2 for READING vendor archives). The correct fix is the upgrade, gated on proof:
  - writer determinism (S15): the conformance corpus and golden `.tsra` bytes/hashes are byte-identical before and after;
  - STORED-only still holds (the memory of #468: STORED held only because `zip` lacked deflate; with ingest's deflate
    feature unified across the workspace, assert the method explicitly in a test if that isn't already done);
  - the reader's range-read/central-directory paths and the cloud (object_store) reader still pass;
  - the feature snapshots are regenerated (`scripts/feature-snapshots.sh`) and the delta explained.
  If the upgrade changes the container bytes, it's a format decision (ADR-0020/0002), not a dependency bump. Then stop and
  decide, and use a documented, time-boxed `deny.toml` ignore justified by non-reachability, tracked here, as the explicit
  interim with an owner sign-off.

Blocks: every open PR's CI Summary (#585, #586, …).

