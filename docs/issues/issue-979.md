---
type: issue
state: open
created: 2026-11-25T00:44:41Z
updated: 2026-11-25T05:57:49Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/979
comments: 0
labels: bug, area:ingest
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-25T10:51:45.613Z
---

# [Issue 979]: [fix(ingest): the sealed ingest_decoder version comes from a lockfile a shared target dir can mix up](https://github.com/vig-os/tessera/issues/979)

## What

The ingest seal records the decoder that read the source (ADR-0056 §6a: `generation.config.ingest_decoder`,
`version` + `features`). `tessera-ingest/build.rs` takes `version` from the workspace `Cargo.lock` text, not from
the decoder crate that is actually linked. Cargo decides whether to re-run that build script from the mtime of the
lockfile path it recorded on its last run. Two checkouts that share one `CARGO_TARGET_DIR` share the build-script
unit, because Cargo leaves a workspace member's path out of its metadata hash. So the second checkout can reuse
`rustc-env` values read from the first checkout's lockfile: the binary links one parquet and seals another version.

## Reproduced (found while cross-checking the corpus for #916)

1. In target slot 1, `cargo clean -p tessera-ingest`, then build and run `gen_ingest_corpus` at dev `dccc60ab`
   (parquet/arrow `=58.4.2`). The output matches the committed corpus.
2. Run the same generator from a `v0.1.0-alpha.6` worktree (parquet/arrow `=58.4.1` in its Cargo.lock) into the
   **same** target dir. That worktree was created before step 1.
3. Its output differs from alpha.6's committed `ingest-corpus.json` in 21 lines: all 7 Arrow fixtures seal
   `"version": "=58.4.2"` and the 58.4.2 feature digest. The 6 Parquet fixtures' `manifest_hash` also differs from
   dev's, because their generated Parquet source still carries the 58.4.1 footer: the product was decoded by 58.4.1
   and sealed as 58.4.2.
4. `cargo clean -p tessera-ingest`, run step 2 again: the output is byte-identical to alpha.6's committed corpus.

No tracked golden is affected (dev's regeneration ran from a clean unit and matches). The seat rules' slot trap
("a sealed ingest_decoder pre-image from the wrong Cargo.lock") is this bug. It can also mislabel a real product
in any build whose target dir is reused across checkouts or worktrees.

## Direction (needs a decision; not done here)

Bind the sealed `version` to the code that is linked, not to a file read at build time. For example, take it from
the decoder crate itself (parquet's `created_by` string, `parquet::file::properties::DEFAULT_CREATED_BY`, names
its version), or fail the build when the lockfile pin and the linked crate disagree. At the least, emit
`rerun-if-changed` for a path that is unique per checkout and stays correct when the target dir is shared.

