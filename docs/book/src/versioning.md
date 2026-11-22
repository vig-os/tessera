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
  - A chunk index is **never copied**, whatever it is called or its class: `--add-block` refuses
    one. When SOURCE carries an index for BLOCK, `commit` **rebuilds** `NAME.cidx` from the copied
    bytes, with the writers' own index functions, and lands it derived. An index is bound to its
    block by name only, so a copied one could claim any statistics; to get one, copy the data block
    from a source that indexes it, or re-write the product. SOURCE must pass `tessera verify`.
  - `tessera stats` and table pruning use an index of `NAME` only when it is the block `NAME.cidx`,
    of kind `chunk_index`, derived, and its `indexes` is `NAME`; otherwise they read the data.
  - A chunk index must index a block of its own product. A product holding one whose block is
    absent is refused by `verify`, by every reader and by `import`, and no writer produces one.
  - `--remove-block NAME` **cascades** to the block's derived family: its derived `NAME.cidx`, and,
    for pyramid level 0 (`BASE/0`), the derived levels `BASE/1…` and their indexes. No accelerator
    outlives the data it was computed from. The summary counts them: `-1 (+2 derived)`. A primary
    chunk index of NAME is never removed implicitly: the remove is refused, naming it, until you
    `--remove-block` it too.
- `log` / `diff` — a lineage's history; what changed between two versions (blocks + metadata).
- `seal` vs `publish` — `seal` exports a standalone `.tsra` *with* its history (a `git bundle`);
  `publish` exports a **history-free** standalone product for distribution (a `git archive`).
- `forget` / `gc` — drop a lineage; reclaim objects unreachable from any ref.

The append-only guarantee is cryptographic, not conventional: a later version's `content_hash` carries a
**consistency proof** that it is an append-only prefix extension of the earlier one (see
[Integrity](./integrity.md)) — you can prove a revision only *added*, never rewrote history.
