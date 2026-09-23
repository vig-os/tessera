# Versioning

Tessera reconciles immutability (content-addressing) with an append-only audit trail through a
**content-addressed repository** — `objects/<digest>` blobs plus `refs`, the same idiom as git, so a new
version costs only its *delta* (unchanged blocks are reused by digest). This is ADR-0036.

The verbs are git-shaped:

{{#include ../../../tessera/crates/tessera-cli/tests/cmd/versioning.trycmd}}

- `init` / `import` — start a repo; import a sealed `.tsra` as the first version of its lineage.
- `commit` — a new version; blocks unchanged since the parent are not re-stored (dedup by digest).
- `log` / `diff` — a lineage's history; what changed between two versions (blocks + metadata).
- `seal` vs `publish` — `seal` exports a standalone `.tsra` *with* its history (a `git bundle`);
  `publish` exports a **history-free** standalone product for distribution (a `git archive`).
- `forget` / `gc` — drop a lineage; reclaim objects unreachable from any ref.

The append-only guarantee is cryptographic, not conventional: a later version's `content_hash` carries a
**consistency proof** that it is an append-only prefix extension of the earlier one (see
[Integrity](./integrity.md)) — you can prove a revision only *added*, never rewrote history.
