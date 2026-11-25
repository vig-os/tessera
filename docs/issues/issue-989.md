---
type: issue
state: open
created: 2026-11-25T04:57:24Z
updated: 2026-11-25T04:57:24Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/989
comments: 0
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-25T10:51:43.231Z
---

# [Issue 989]: [ingest_decoder pins read tessera's lockfile when tessera-ingest is a git/path dependency of another workspace](https://github.com/vig-os/tessera/issues/989)

## What

The sealed `ingest_decoder` triple (ADR-0056 §6a) reads decoder versions and decode-path pins from
`tessera-ingest/../../Cargo.lock`. When tessera is a member of the workspace being built, that file is the
resolution cargo built from. When another workspace depends on `tessera-ingest` as a **git** or **path**
dependency, `../../Cargo.lock` is still *tessera's* lockfile, while cargo resolved the *consumer's*. A pin can
then name a version the consumer's binary did not link (a transitive crate on a decode path, or a lane root if
the consumer overrides it with `[patch]`).

This is older than #979: before it, `build.rs` walked up to the same file. The #979 fix (the resolution read in
the library compile unit) documents it as a limit in `tessera-ingest/src/linked.rs` and the ADR-0056 §6a #979
amendment.

## Why it is not fixed in #979

Cargo gives a crate no stable way to learn which workspace (and lockfile) it is being built for:
`CARGO_MANIFEST_DIR` names the package, not the workspace root, and there is no stable `CARGO_WORKSPACE_DIR`. A
layout heuristic (`$CARGO_HOME/git/checkouts/…`) would miss path dependencies and is a pattern-match, not a fact.

## Options to decide between

- Record nothing (cfg off) unless the build can prove tessera is the root workspace, for example via an env
  var set in tessera's own `.cargo/config.toml` `[env]` (`relative = true`), which a consumer's build does not
  load.
- Use cargo's `CARGO_WORKSPACE_DIR` / `-Zlockfile-path` equivalents once stable.
- Accept it and document it as unsupported for provenance-grade ingest.

