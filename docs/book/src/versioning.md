# Versioning

Tessera reconciles immutability (content-addressing) with an append-only audit trail through a
**content-addressed repository** — `objects/<digest>` blobs plus `refs`, the same idiom as git, so a new
version costs only its *delta* (unchanged blocks are reused by digest). This is ADR-0036.

The verbs are git-shaped:

{{#include ../../../tessera/crates/tessera-cli/tests/cmd/versioning.trycmd}}

- `init` / `import` — start a repo; import a sealed `.tsra` as the first version of its lineage.
- `commit` — a new version; blocks unchanged since the parent are not re-stored (dedup by digest).
  - `--add-block [NAME=]SOURCE.tsra:BLOCK` copies an encoded block from another product. The copy
    is **primary**, part of the new version's `content_hash`, whatever its class in SOURCE: a
    derived block is derived of the product that computed it, and is data anywhere else.
  - A derived block is never copied. When SOURCE carries an index for BLOCK, `commit` **rebuilds**
    `NAME.cidx` from the copied bytes, with the writers' own index functions, and lands it derived.
    The source's index bytes are never trusted: an index is bound to its block by name only, so a
    copied one could claim any statistics. A `.cidx` copied explicitly is primary data under its
    own name (`other.cidx=SOURCE.tsra:volume.cidx`); one that would take the name of an index being
    rebuilt is refused.
  - `--remove-block NAME` **cascades** to the block's derived family: its `NAME.cidx`, and, for
    pyramid level 0 (`BASE/0`), the levels `BASE/1…` and their indexes. No accelerator outlives
    the data it was computed from. The summary counts them: `-1 (+2 derived)`.
- `log` / `diff` — a lineage's history; what changed between two versions (blocks + metadata).
- `seal` vs `publish` — `seal` exports a standalone `.tsra` *with* its history (a `git bundle`);
  `publish` exports a **history-free** standalone product for distribution (a `git archive`).
- `forget` / `gc` — drop a lineage; reclaim objects unreachable from any ref.

The append-only guarantee is cryptographic, not conventional: a later version's `content_hash` carries a
**consistency proof** that it is an append-only prefix extension of the earlier one (see
[Integrity](./integrity.md)) — you can prove a revision only *added*, never rewrote history.
