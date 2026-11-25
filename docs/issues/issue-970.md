---
type: issue
state: closed
created: 2026-11-24T19:22:42Z
updated: 2026-11-24T21:07:46Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/970
comments: 0
labels: none
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-25T10:51:48.772Z
---

# [Issue 970]: [cargo-deny: RUSTSEC-2026-0595 (parquet 58.4.1) fails Lint on every branch](https://github.com/vig-os/tessera/issues/970)

The `cargo-deny (licenses + advisories)` pre-commit hook now fails on every branch. Lint & Format is red, so CI Summary is too.

```
error[unsound]: Apache Arrow Rust: Bloom filters written as plaintext when writing encrypted Parquet
  tessera/Cargo.lock:340  parquet 58.4.1
  ID: RUSTSEC-2026-0595
```

The advisory was published after dev's last green CI (dde0517f). It first showed up on PR #960 at 5a9a44de, a rebase onto 3b022960 that does not touch dependencies (Lint job 138676065992).

Fix: bump `parquet`/`arrow` to a patched release, or, if tessera never writes *encrypted* Parquet with bloom filters, add a justified `ignore` entry in `tessera/deny.toml` the way the existing RUSTSEC entries do. Until this lands, every open PR is blocked.

