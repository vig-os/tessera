# ADR-0063 — Block names: a hierarchical `/`-segment grammar, and blocks are the leaves

**Status:** Accepted (owner decision 2026-10-29,
[#685 comment](https://github.com/vig-os/tessera/issues/685#issuecomment-6486012695)). Fixes the path
traversal in [#685](https://github.com/vig-os/tessera/issues/685). Refines the `BlockRef.name` field of
SPEC §5, which was `string` with no rule. Keeps ADR-0043's hierarchy (pyramid levels, sidecars and
collection members are nodes of one tree); keeps ADR-0022's layouts (sealed zip, exploded directory, S3
prefix) mirroring one another. Amended by ADR-0064 (2026-10-30): a node of numbered, marked table leaves is one
logical table.

## Context

A block's name is more than a label. It is the key of the `.tsra` zip member `blocks/<name>`, the file
`blocks/<name>` of an exploded directory (`unpack`/`pack_dir`, ADR-0022), a write session's fragment
`blocks/<name>` (ADR-0026), and an object-store key prefix. Until now the format said only
`name: string`, and nothing checked it:

- `WriteSession::append_block` created its fragment at `dir.join("blocks").join(&name)`, and `recover`
  followed a journal line's name the same way. With `Path::join`, `../x` climbs out of `blocks/`, and an
  absolute name replaces the base entirely. A name taken from caller input (an ingest spec,
  `commit --add-block`, the Python and stream APIs) could create or overwrite any file the process can
  write (#685).
- `unpack` wrote `outdir/blocks/<name>` and `pack_dir` read `dir/blocks/<name>` for names taken from the
  manifest of an untrusted `.tsra`, and `unpack` did the same for its `aux/` members.
- `tessera pyramid` already wrote hierarchical names: level 0 as `volume`, the downsampled levels as
  `volume/1`, `volume/2`, …. Such a product reads in-zip, but `unpack` failed on it, because `volume`
  can't be both a file and the directory that holds `volume/1`.

The corpus and every fixture use plain names (`volume`, `events_0007`, `volume.cidx`, `file_0000`,
`gen-01`, `DP06-ct`). The pyramid is the only `/` user.

## Decision

### 1. Grammar

```text
name    = segment *( "/" segment )            ; at most 255 bytes in total
segment = 1*( ALPHA / DIGIT / "." / "_" / "-" ) ; ASCII only; not "." and not ".."
```

So a name has no leading or trailing `/`, no empty segment (`//`), no `.` or `..` segment, no `\`, `:`,
NUL, space or non-ASCII character, and is at most 255 bytes long. The ABNF is normative; as a regex
(matching the whole string):
`^(?!.{256})(?:(?!\.\.?(?:/|$))[A-Za-z0-9._-]+)(?:/(?!\.\.?(?:/|$))[A-Za-z0-9._-]+)*$`.

### 2. Leaf rule

Within one product, no block name may be a parent prefix of another: `volume` and `volume/1` can't both
exist, but `volume/0` and `volume/1` can. Blocks are the leaves of the name tree. That keeps the zip
members, the exploded directory and an S3 prefix one-to-one with the names, since a name is then either
a file or a directory, never both. The rule is a property of the set of names, so it is checked where a
set is finalised (§4), not on a single name.

### 3. `tessera pyramid` names level 0 `<block>/0`

It wrote `<block>`. Now every level, the full-resolution one included, is a leaf under `<block>/`.

### 4. Where it is enforced

- **One type.** `tessera_core::block::BlockName` holds a name that passes the grammar. `BlockRef.name`,
  `BlockPayload.name` and the `pack_streaming` sources are `BlockName`, so an invalid block reference
  can't be represented. Deserializing one (a manifest, a journal line) applies the grammar too. It
  serializes as the plain string, so no manifest byte, `content_hash` or corpus golden moves.
- **The filesystem boundary.** `BlockName::path_under(base)` is the only way a block name becomes a
  path. It pushes plain (`Component::Normal`) components onto `base` and refuses anything else. The
  grammar already rules anything else out on every platform `std` supports (a segment can't hold a
  separator, a drive or verbatim prefix, `.` or `..`); the boundary checks again rather than trusting
  the grammar alone. `aux/` member names are joined through the same grammar
  (`block::name::join_checked`). `write_aux_members` refuses an aux name that fails it, and so does
  `pack_dir`, before writing; rewriting an archive refuses a `blocks/` or `aux/` entry it would copy
  through that fails it.
- **Orphan fragments.** `WriteSession::recover` deletes every file under `blocks/` that no line of the
  journal names, so a fragment left by an append that never reached its journal line can't block a
  later append whose name needs that path as a directory. A fragment some line names is kept even when
  recovery stops before that line. *(Amended by #686, below: such a fragment is quarantined, not left
  in `blocks/`.)*
- **Writers enforce the leaf rule.** `ProductBuilder::seal` refuses a conflict, and so does a write
  session (on each append, against the blocks it has committed, and in `recover` against earlier
  journal lines). `unpack` and `pack_dir` refuse a conflict too, before they write anything.
- **Readers accept any grammar-valid name.** `Reader::open`, `verify`, `read_block` and the other
  in-zip reads don't apply the leaf rule. A pyramid sealed before this ADR (`volume` + `volume/1`)
  still opens, verifies and reads. It can't be exploded to a directory, and `unpack` says so with the
  typed conflict. `pack` and `pack_streaming` package a manifest that is already sealed, so they accept
  any sealed manifest, including such a pyramid.
- **Typed errors.** `Error::InvalidBlockName { name, reason: NameError }` names the rule broken (empty,
  too long, empty segment, dot segment, disallowed character). `Error::BlockNameConflict
  { parent, child }` reports a leaf-rule conflict. `recover` returns these for a complete journal line;
  a torn last line (not complete JSON) is still dropped as before. *(Amended by #686, below: the torn
  tail is the unterminated last line, and any other invalid complete line is `Error::CorruptJournal`.)* Reading a manifest with an invalid
  name fails to parse (`Error::Serde`, message `invalid block name '<name>': <reason>`).

## Compatibility

Products whose names all pass the grammar and the leaf rule are unaffected, byte for byte; that is the
whole corpus and every fixture (pinned by `tessera-io/tests/block_names.rs::every_corpus_and_fixture_name_validates`).
An older pyramid (`volume` + `volume/1`) still reads in-zip but no longer explodes; re-run
`tessera pyramid` to get the `volume/0` layout. A product with a name outside the grammar (a space,
non-ASCII, `../`) is refused everywhere. No such product is known; one would come from a caller that
named a block freely through `commit --add-block`, the Python builder or an ingest spec.

`aux/` member names follow the same grammar. An archive carrying an aux member outside it (an older
`pack_dir` embedded any file name, `aux/my notes.txt`) still opens and reads, but `unpack` refuses it,
and rewriting it (`tessera sign`) refuses unless that member is the one being replaced or removed.

## Alternatives rejected

- **A single segment (`[A-Za-z0-9._-]+`, no `/`).** It gives up the hierarchy ADR-0043 builds on and
  would rename every pyramid level.
- **A flat on-disk encoding** (escape `/` in a file name, keep names free-form). The exploded and S3
  layouts would stop mirroring the names, and every tool that lists them would need the encoding.
- **Checking each `join` call site separately.** Every new call site would have to remember the check,
  which is how #685 happened. The type and the single `path_under` make that unnecessary.

## Security rationale (#685)

Joining an untrusted name onto a directory is the classic path-traversal (zip-slip) bug, on both sides:
a writer handed a name by its caller, and a reader handed a name by a file. Validating once into a type
removes the class rather than the instance. Every block-name path in the code goes through
`path_under`, and every entry name this crate writes into a `.tsra`, including the `blocks/` and
`aux/` entries it copies through when it rewrites one (`write_aux_members`, used by `tessera sign`),
obeys the grammar, so it unzips under its prefix.

## Limitations

- **Windows reserved device names** (`CON`, `NUL`, `AUX`, `COM1`, …) are grammar-valid. On Windows,
  writing one as a file name opens a device. Tessera ships no Windows build today (`dist-workspace.toml`
  defers Windows-msvc), and [#690](https://github.com/vig-os/tessera/issues/690) tracks refusing them
  in `path_under` before a Windows build ships.
- **Case-insensitive filesystems** (macOS default, Windows) treat `Volume` and `volume` as one file, so
  exploding a product holding both would collide. The leaf rule doesn't cover case; #690 tracks it too.

## Amendment (#686, 2026-10-29) — recovery quarantines what it can't replay, and a complete line is a commit

The orphan rule above kept the fragment of a journal line that recovery stopped before, so that a journal
cut short could be repaired by hand. It did not hold: `recover` rewrote the journal to the replayed
prefix and then removed orphans, so a second `recover` no longer saw the line and deleted the fragment;
and a kept `c/0` directory made a later append of `c` fail with an I/O error (EISDIR) instead of a typed
one. `WriteSession::recover` now does this instead (rustdoc on `recover` is the reference):

- **Torn tail.** Only an unterminated last line is torn, and it is dropped whatever its bytes (a cut-off
  prefix, a whole object whose newline never reached the disk, bytes cut inside a UTF-8 character). Every
  complete line is a commit: one that is not UTF-8, not JSON, not a block reference, or that commits a
  block a second time is refused with `Error::CorruptJournal { path, line, detail }`, before anything on
  disk changes, like the grammar and leaf-rule errors above.
- **Quarantine.** When complete lines follow the first line whose fragment is missing, that line and all
  after it move, verbatim, to `quarantine/<blake3 hex of those lines>/journal.jsonl` in the stage
  directory, and the fragments they name to `quarantine/<id>/blocks/`. The quarantine is outside
  `blocks/`, so it is never sealed and never in the way of a path. The names it holds stay taken: an
  append whose name is one of them, or a parent or child of one (the leaf rule), is refused with
  `Error::QuarantinedBlockName { name, quarantined, quarantine }`. Repairing by hand (put the fragments and
  lines back, delete the quarantine, recover) or deleting the quarantine frees them. `create` on the same
  directory keeps the quarantine, and its names stay taken. A lone last line whose fragment is missing has
  nothing left to keep and is dropped, as before: usually the commit whose journal fsync failed, but an
  acknowledged commit whose fragment was lost to damage to `blocks/` is lost with it.
- **Strictness.** A crash that leaves stale bytes ending in a newline at the journal's end (possible where
  the filesystem journals metadata but not data) now makes `recover` refuse the session rather than guess
  which lines are commits.
- **Durable, idempotent.** The journal is rewritten through the session's one durable-write path (temp
  file, fsync, atomic rename, directory fsync), so a crash during recovery leaves the old journal or the
  new one. The quarantine's id is computed from its lines, so a crash part-way through quarantining is
  completed by the next recovery into the same directory. Recovering a recovered session changes
  nothing.

This is internal to the staging directory, not the sealed format: no grammar, leaf-rule, manifest or
`content_hash` change, and no corpus golden moves.

## Amended by ADR-0064 (2026-10-30) — a node of numbered, marked leaves is one logical table

[ADR-0064](0064-logical-tables-over-numbered-leaves.md) (Proposed) gives one kind of node a meaning. §2 above
says only that blocks are the leaves of the name tree; that stays true, and the leaf rule doesn't change.
Added:

- **A partitioned table is a node.** A table of more than `BLOCK_ROWS` rows is written as the leaves
  `<table>/0000`, `<table>/0001`, …: the leaf of index `i` is `i` padded with `0` to at least four digits
  (`<table>/10000` follows `<table>/9999`). Each leaf's `TableSpec` carries a sealed partition marker
  `{"index": i, "last": b}`. The node `<table>` is then **one logical table**, the leaves in index order, and
  every logical reader answers to `<table>` (ADR-0064 §3, §7).
- **Such a node holds nothing else.** A node with at least one marked block exactly one segment below it holds
  only its marked `table` leaves (each named by the canonical padded form of its own index) and their chunk-index
  sidecars `<table>/<leaf>.cidx`. The indices run `0..=k` with `k ≥ 1`, without a gap or a duplicate, and exactly
  the leaf `k` is `last`. A foreign child, a gap or a second terminal makes the node invalid, and so does a marker
  on a block with no parent node: readers refuse to resolve it with `Error::LogicalTable`, and the points that
  check the leaf rule (§4: `ProductBuilder::seal`, a write session's seal, `unpack` and `pack_dir`) refuse it.
  A block under such a node stays readable by its full name.
- **Nodes without markers are unaffected.** `tessera pyramid`'s `volume/0`, `volume/1`, … (§3) carry no marker,
  so they are array levels, not a logical table, and nothing about them changes.

No grammar, leaf-rule or existing-manifest byte changes: every name in the corpus and the fixtures still
validates, and no corpus golden moves.
