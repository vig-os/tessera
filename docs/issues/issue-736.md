---
type: issue
state: closed
created: 2026-11-02T12:53:20Z
updated: 2026-11-02T16:12:03Z
author: gerchowl
author_url: https://github.com/gerchowl
url: https://github.com/vig-os/tessera/issues/736
comments: 0
labels: bug
assignees: none
milestone: none
projects: none
parent: none
children: none
synced: 2026-11-03T08:59:45.225Z
---

# [Issue 736]: [deps: RUSTSEC-2026-0435 (thrift 0.17 via parquet 58) fails cargo-deny on every PR](https://github.com/vig-os/tessera/issues/736)

**cargo-deny fails on every PR:** the advisory RUSTSEC-2026-0435 (CVE-2026-43868, published since `dev` was last green) flags `thrift` 0.17.0. It is an unbounded allocation in `TBinaryInputProtocol::read_bytes`, fixed in `thrift` 0.23.0.

- The dependency path is `thrift v0.17.0 ← parquet v58.3.0 ← tessera-ingest, tessera-cli` (`cargo tree -i thrift@0.17.0`).
- First seen in PR #735's `Lint & Format` (run 41447415597, `cargo-deny … advisories FAILED`). That PR doesn't touch dependencies, so every open PR fails the same way until this is resolved.
- The decision is: upgrade (does a parquet release move to `thrift` ≥ 0.23, given the arrow pin through the vortex 0.75 fork, as in #683?) or an audited `deny.toml` ignore. The ignore needs a reachability argument: parquet's metadata decoding uses thrift's **compact** protocol, so check whether `parquet` 58 reaches `TBinaryInputProtocol::read_bytes` at all. Either way it needs a removal issue, as with the existing ignores.

