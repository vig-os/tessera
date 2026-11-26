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

## Concurrent writers and crashes

A repository has **one writer at a time**: every command that changes it (`import`, `commit`, `forget`, `gc`)
holds an exclusive lock on the file `repo/lock` for its whole run, and others wait for it. Readers (`log`, `diff`,
`publish`, `seal`) never wait.

- `commit` builds the new version on the tip it read. If another commit landed on the lineage in between, it is
  refused with `ref '…' moved: expected …, found …` and nothing is written: run it again, on the new tip. No commit
  that reported success is ever dropped from the lineage's history.
- `import` starts a lineage. Importing into a lineage whose tip is another version is refused the same way; use
  `commit` to change it, or `forget` it first. Importing the tip's own version again changes nothing, and rewrites
  any of its objects that are missing or torn.
- Objects are written to `repo/tmp/`, synced and renamed into place before the ref names them, and the ref is
  replaced the same way, so a crash or power loss leaves each lineage at its old version or its new one, never at a
  version with missing data. An object a crash left torn is detected by its hash and rewritten by the next write of
  the same bytes.
- `gc` refuses to delete anything if it cannot read a ref or a manifest it reaches, and names it: a walk that
  skipped it would delete what only it keeps alive. An empty or garbled ref is an error, never "no commits yet";
  restore it, or `forget` the lineage.
- The lock is an advisory `flock`, valid on a **local filesystem only**: do not share one repository between hosts
  over NFS or another network filesystem without lock support. (A repository on an object store would fence with a
  conditional put; it is not built yet, #288.)

## Lineage names and ref files

A lineage is named by its product `id` (`blake3:<hex>`, printed by `import`), and its tip lives in the file
`repo/refs/<name>`, with its history cache at `repo/log/<name>.jsonl`. The file name is the lineage **percent-encoded**:
every byte outside `a-z`, `0-9`, `-` and `_` is written `%` plus two lower-case hex digits, so `blake3:3f…` is stored as
`blake3%3a3f…`, `a:b` as `a%3ab` and `Ab` as `%41b`. Distinct lineages never share a file, on a case-insensitive file
system too (macOS by default). An empty lineage, or one whose encoded name is longer than 249 bytes, is refused with
`lineage '…' cannot name a ref`; a product id needs 73.

The root file `repo/layout` records this naming (layout version 1). A repository made before it (tessera
0.1.0-alpha.7 and earlier) named `a:b` and `a_b` both `a_b`, so a commit on one moved or blocked the other (#975).
Every verb refuses such a repository and never reads it with the new naming:

```text
repository repo has the layout the one from before #975 (…), and this build reads only `tessera-repository 1` (…);
there is no migration: create a new repository (`tessera init`) and import the source products again
```

There is no migration verb: an alpha carries no backwards-compatibility baggage, as for products (ADR-0066). Create a
new repository with `tessera init` and `import` the sealed source products into it again.

The append-only guarantee is cryptographic, not conventional: a later version's `content_hash` carries a
**consistency proof** that it is an append-only prefix extension of the earlier one (see
[Integrity](./integrity.md)) — you can prove a revision only *added*, never rewrote history.
