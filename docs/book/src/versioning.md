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
`repo/refs/lineages/<name>`, with its history cache at `repo/log/lineages/<name>.jsonl`. The file name is the lineage
**percent-encoded**:
every byte outside `a-z`, `0-9`, `-` and `_` is written `%` plus two lower-case hex digits, so `blake3:3f…` is stored as
`blake3%3a3f…`, `a:b` as `a%3ab` and `Ab` as `%41b`. Distinct lineages never share a file, on a case-insensitive file
system too (macOS by default). An empty lineage, or one whose encoded name is longer than 249 bytes, is refused with
`lineages ref '…' cannot name a file`; a product id needs 73.

The root file `repo/layout` holds exactly `tessera-repository 2` followed by a line feed. Each ref holds exactly
`<kind> SP blake3:<64 lower-case hex> LF`: one space, one final line feed, no other whitespace or trailing bytes.
Kind is `product` or `collection`. A missing line feed, upper-case hex or an unknown kind is a corrupt ref.
Every ref read checks its record kind against the object, refusing a disagreement with `RefKindMismatch`,
and verifies the target's stored bytes and seal. Bare blocks cannot be ref targets.

The Rust repository API stores sealed collections with `put_collection` and reads them by seal with
`get_collection`. Both products and collections use the same content-addressed object store and fenced writer.
`set_ref` infers the record kind from the stored target. An existing lineage keeps its recorded kind:
`commit` and `set_ref` refuse a change with `LineageKindMismatch` under the writer lock, before storing anything.
Typed object accessors use `ObjectKindMismatch` when the requested kind disagrees with the object; ref-record
mismatches remain `RefKindMismatch`. These are separate low-level calls: until a live ref or
collection reaches a newly stored object, `gc` can reclaim it. Use `commit_collection` for an atomic collection commit.

`Repository::commit_collection(lineage, expected, &collection, &members)` validates member closure and
stores supplied `NewMember::Product { manifest, payloads }` and `NewMember::Collection(&child)` values
under the same writer lock that advances the ref. Supply members in any order. Every nested member must be
stored or supplied, readable as its declared kind, with each product block stored or supplied as a verified
payload. Unchanged blocks are checked for existence only. `MissingMember` names the failing member path.
CAS failure (`RefMoved`), lineage kind changes (`LineageKindMismatch`), a wrong supersedes pin
(`SupersedesMismatch`) and a changed lineage id (`LineageIdMismatch`) all refuse before storing objects.
Later versions keep their parent's `id_inputs`; forks add a unique non-empty `fork` token and a
`forked_from` edge pinning the origin version's seal. These rules reuse core sealing validation.

`log(lineage)` reads the same oldest-first `{manifest_hash, parent}` cache for either kind. The cache can
be incomplete or torn after a crash; repair is tracked in #971. `history(address)` reads sealed ancestry
from any product or collection version and includes the starting version. Each record carries `address`,
`kind`, `id` and `predecessors` (role and target seal). It visits supersedes before forked_from in depth-first
preorder, preserving source order within each role and returning shared ancestors once. Members and other
source roles are excluded. A missing or unreadable predecessor returns `HistoryAborted` naming its address
and incoming edge; long chains use bounded error paths. This is stricter than GC's open-edge rule: a local
history answer must be complete. Readers take no lock, so concurrent forget/GC may cause a read error.
A log append failure can be reported after a successful ref advance; inspect the tip before retrying.

`gc` marks through nested collection members and both kinds' `sources` edges. Members are closed: a missing,
unreadable, torn or wrong-kind member aborts before any deletion, naming the full path from the ref through each
`members[index]`. Missing edge targets stay open (held elsewhere). Shared versions are read once, every incoming
kind claim is checked, and traversal uses a heap stack with no depth limit. Product blocks are marked without
reading their payloads. Traversal provenance stores parent indices instead of copying path prefixes, so its
bookkeeping grows with the reachable graph. Paths are rendered only on failure: all member steps remain visible,
while consecutive source edges are summarized by a version/object count. A long history therefore has a small
error message. The cost is the reachable version graph plus the listing of all objects.

Pins have their own directory, `repo/refs/pins/<name>`, and use the same encoding, bound and typed record grammar.
A lineage and a pin may share a name: lineage commands only touch `refs/lineages/` and `log/lineages/`.
`gc` reads roots from both namespaces. Pin management commands are planned in ADR-0067 step 6.
A stray entry under `refs/`, a non-regular file inside either namespace or a malformed record aborts `gc` before
anything is deleted.

Every verb refuses repositories at layout 1, without a marker or at any other layout. The error names the layout
found and the required layout, 2. `open` refuses even an empty unmarked directory; only `init` creates the marker,
in a directory without ref, log or object entries. `init` also refuses existing repositories at other layouts.

There is no migration verb. Use the build that wrote the old repository to `seal` each lineage tip into a
self-contained `.tsra`, then `init` a new repository with the current build and `import` those products into it.
History edges travel with the sealed versions; the old repository stays unchanged. Builds from #990 onward
refuse layout 2. Earlier builds do not check the marker and their `gc` can empty a layout-2 store: never run them
against it.

The append-only guarantee is cryptographic, not conventional: a later version's `content_hash` carries a
**consistency proof** that it is an append-only prefix extension of the earlier one (see
[Integrity](./integrity.md)) — you can prove a revision only *added*, never rewrote history.
