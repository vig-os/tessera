# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## Unreleased

### Added

- `tessera-core`: one manifest-only resolver for a table name, `block::table_group`, with the typed
  `Error::LogicalTable` and its `LogicalTableError` reasons, and the seal-time check `block::check_partitions`, which
  `ProductBuilder::seal` now runs (ADR-0064 PR A, #709).
- `tessera-io`: `Reader::table_group` resolves a table name through that resolver, and `Reader::logical_table`
  (`LogicalTableView`) is built on it, so it reads a partitioned table (`pairs/0000`, `pairs/0001`, …) as one table
  as well as a single block or a legacy `events_NNNN` group (ADR-0064 PR B, #710). `tessera-core` adds
  `block::group_specs`, the one schema check for a resolved group, and `TableGroup::blocks`.
- `tessera-py`: `Reader.table`, `table_arrow`, `table_dict` and `column` (and the native `read_table` /
  `read_table_column`) take a logical table name, so a partitioned table (`pairs/0000`, `pairs/0001`, …) reads as
  one table, every leaf's rows in leaf order, as a legacy `events_NNNN` group does (ADR-0064 PR C2, #717). A name
  that is not a block passed to `read_block`, `array` or `array_roi` raises the new `tessera.NotABlockError`, a
  subclass of `TesseraError`.
- `tessera-cli`: `ls` and `tree` show a partitioned table as one node, `pairs/  table, 3 blocks, 8 rows`, where they
  listed each leaf; `ls FILE pairs` lists its columns and leaves, and `tree --full` lists the leaves under the node.
  An invalid partitioned table shows its typed error instead. `read` and `sql` answer `pairs` as one table, and
  `read` on a node that holds no table says so and points to its blocks (ADR-0064 PR C1, #711). Legacy
  `events_NNNN` blocks are listed one per line, as before.
- `tessera-io`: `StreamWriter::push_table(name, spec, groups)` writes a streamed table of any size as one logical
  table, partitioned at `BLOCK_ROWS` into the marked leaves `name/0000`, `name/0001`, …, byte for byte the leaves
  `TableMultiBlockSink` writes for the same rows (ADR-0064 PR E, #713); at most `BLOCK_ROWS` rows it is the one block
  `name`. Each block's input is staged on disk in the session's `staging/` directory and handed to a free encode
  worker once complete, so at most `W + 1` blocks of input are staged for `W` workers, counted by the new
  `StreamStats::peak_staged_input_blocks` and `peak_staged_input_bytes`. A row count that disagrees with
  `spec.rows`, or a row-group above `ROWS_PER_GROUP` rows, is `Error::StreamedTable` with a typed
  `StreamedTableError` reason (`tessera-core`).
- `tessera-io`: `StreamWriter::push_table` documents its heap bound, `min(W, n) · p_block + s + c_leaf · n` for `W`
  encode workers and `n` leaves, with `p_block` one leaf's measured write peak, `s = 2 · ROWS_PER_GROUP · r_mem + 4 MiB`
  (7 MiB for 24-byte rows) and `c_leaf = 2 · (e_leaf + b_leaf)` for the manifest entry each committed leaf keeps until
  the seal (`e_leaf` the entry's canonical JSON length, `b_leaf = 1 KiB`; ADR-0064's #760 amendment, holding once
  #759 lands). Beyond that per-leaf term the bound doesn't grow with the row count. The rustdoc also says that staged
  input is page cache, which counts toward a cgroup's memory and is RAM on tmpfs (ADR-0064 PR F, #714).
  `tests/staged_job_memory.rs` checks the bound on the heap (without the per-leaf term, at most 25.3 KB at its 8
  leaves), and that neither the reader filling a block nor a worker reading it back holds the block in RAM.
- **Breaking for `TableSpec` struct literals, `tessera-core` / `tessera-io`:** `TableSpec` gains
  `edition: Option<String>`, the Vortex edition a table block's payload was written under (ADR-0024's 2026-11-15
  amendment, #635). `tessera-io`'s writer sets it to `table::TABLE_EDITION` (`"core2026.08.3"`) on every table block
  and partition leaf it seals, and refuses a spec that names another edition with `Error::Codec`. Build specs with
  `edition: None`. It is in the manifest, not the payload, so it moves `manifest_hash` and leaves `content_hash` and
  the block digests as they are; the corpus goldens are regenerated with it, in the same format event as the vortex
  0.93 bump below. A spec without the key (every product sealed before this change) still parses, reads and
  verifies, and keeps its manifest bytes: `None` is omitted, never written as `null`. The key itself doesn't stop an
  older reader, because `BlockRef.spec` is kept as raw JSON. But the table bytes it marks do: an older reader can't
  decode them (the forward-read limit under **Changed** below). `TableSpec` has no `Default`
  and isn't `#[non_exhaustive]`, so a struct literal outside tessera stops compiling until it names the field.
- **Breaking for `TableSpec` struct literals, `tessera-core` / `tessera-io` / `tessera-cli`:** `TableSpec` gains
  `codec_profile: Option<String>`, the tessera codec profile a table block was written with (ADR-0024's 2026-11-16
  amendment, #869): the codec set (which of the edition's schemes the compressor may choose from) and the pinned
  Vortex compressor, named `tessera-table-v<N>`. `tessera-io`'s writer records `table::TABLE_CODEC_PROFILE`
  (`"tessera-table-v1"`) beside the edition on every table block, partition leaf and sequenced-table leaf, and refuses
  a spec that names another profile with `Error::Codec`. Build specs with `codec_profile: None`. With the edition it
  makes "same input + same edition + same profile → same bytes" checkable from the product alone; it says nothing
  about decodability, and readers never refuse a block whose profile is unknown or absent (every product before
  this change). It is in the manifest, not the payload, so it moves `manifest_hash` of every table-bearing product
  and leaves `content_hash` and the block digests as they are; the goldens (`corpus/corpus.json`, `corpus/files`,
  `corpus/ingest-corpus.json`, the `logical_tables` walkthrough's `scan.tsra`, tessera-py's `partitioned_pairs.tsra`)
  are regenerated. `tessera inspect` prints `edition <id>  codec_profile <id>` under each table block, `-` for an
  absent key. A test pins the table compressor's configuration against the profile, so a codec-set change that
  doesn't bump the profile fails CI.
- `tessera-io`: `StreamWriter::open_sequenced_table_on` opens a `SequencedTable`, a table lane that any number of
  threads push numbered batches into (`push(&self, writer, seq, batch)`, `mark_done`, `close(expected_count)`), sealed
  byte for byte as the batches in `seq` order whatever order they arrive in (ADR-0065 §7.2–§7.3, #812). Its gate is a
  second credit window of the writer: the due `seq` is never held, others wait while `seq >= next + window` or the
  shared byte credit (the writer's `max_in_flight_bytes`, the encode window's budget, so a caller holding a RAM budget
  sets it to half) is held, or are refused in `refuse_instead_of_block` mode. A `seq` below `first_seq`, a
  duplicate, a `seq` past `max_ahead`, a gap or count mismatch at close, `abort`, the finite stall deadline (default
  10 minutes, at most `MAX_STALL_TIMEOUT` = 24 hours) and a dead pipeline each wake every waiting push with an error.
  Every refusal poisons the writer: its other pushes return it, and `finish` returns it and seals nothing, as it does
  for a sequenced table still open or dropped unclosed (`NotClosed`). `SequencedOptions` and `SequencedStats`
  configure and observe it. `tessera-core` adds the `StreamedTableError` reasons `Duplicate`, `BeforeFirst`,
  `TooFarAhead`, `WouldBlock`, `Missing`, `CountMismatch`, `Aborted`, `Stalled`, `Failed` and `NotClosed`.

### Changed

- `tessera-core`: newer-field paths (`Manifest::newer_fields`, `Collection::newer_fields`, `ProductSchema::view_of`,
  `Error::NewerFormatFields`) quote a key that isn't a plain identifier, `["producer.agents"]`, so no two paths read
  the same (SPEC §4); code that parses these strings sees the new form. Plain paths are unchanged (#873, see
  Security).

- **Breaking, `tessera-core`:** `ProductBuilder::from_manifest` returns `Result` and refuses a parsed manifest with
  `Manifest::newer_fields()` (keys from a newer format revision, verified but not carried by the struct) with the
  new `Error::NewerFormatFields`, as do `Manifest::compute_manifest_hash`, `canonical_bytes` and `verify` on such a
  view. `Manifest::to_sealed_json` is the JSON every writer now emits (`pack`, `pack_streaming*`,
  `Repository::put_manifest`): it refuses a manifest whose JSON doesn't reproduce its own seal. `unpack`, `pack_dir`
  and the Python `Reader.verify` use the stored manifest bytes (`Reader::manifest_json`), so such a product
  round-trips unchanged. `tessera verify` warns about newer fields. `Manifest` gains a private field, so it can no
  longer be built with a struct literal outside `tessera-core` (use `Manifest::new`). `Manifest::from_json`
  (and `read_manifest_unverified`) refuse duplicate keys too. The wasm `verify_signature` now verifies the
  manifest's seal before the signature (#573).
- **Breaking, `tessera-core`:** `Collection::compute_manifest_hash`, `canonical_bytes` and `verify` refuse a parsed
  collection with `Collection::newer_fields()` (`Error::NewerFormatFields`) rather than seal a view that drops them.
  `Collection` gains a private field, so it can no longer be built with a struct literal outside `tessera-core` (use
  `Collection::new` or `CollectionBuilder`). `Collection::from_json` refuses duplicate keys. The new
  `Collection::to_sealed_json` is what `tessera collection new` and the ingest engine write: it refuses a view whose
  JSON doesn't reproduce its seal. `Collection::from_json_with_seal` returns the seal over the object as received.
  The message of `Error::NewerFormatFields` now also covers validation (#873).
- **Format event (table blocks), breaking for writer output, for `tessera-io` sink types and for old readers
  (releases up to `0.1.0-alpha.5` can't decode tables written by this version):** vortex moves from 0.75 to
  0.93.0, pinned to the fork `gerchowl/vortex@5c55cf7` (0.93.0 plus the #468 fix, which upstream hasn't merged;
  the #472 fix shipped upstream in 0.89.0). Vortex 0.93 writes column statistics as a `vortex.zoned` zone
  map instead of `vortex.stats`, with no `sum` or `*_is_truncated`. Column encodings are unchanged. So the sealed
  bytes of **table** blocks change, and re-ingesting or re-building the same input gives a new `content_hash` and
  `manifest_hash` for every product that holds one. Table blocks are 5–10 % smaller. Array, blob and table-free
  products are byte-identical. Products written by 0.75 still open, read and verify. **Not forward-compatible:**
  table blocks written by this version need a tessera reader at or after this change, which links Vortex ≥ 0.85, the
  minimum for `TABLE_EDITION`. Their `TableSpec.edition` names the edition they need. A tessera reader from
  before it (`0.1.0-alpha.1`–`alpha.5`) still opens and verifies such a product. But it fails to decode the table:
  `tessera read FILE <table>` exits 1 with `codec: Other error: Encoding not found in registry: 1`. Upgrade every
  reader before publishing products sealed by this version (SPEC §5b; a clearer error is #871). The table writer
  enables the frozen vortex edition `table::TABLE_EDITION` (`core2026.08.3`) and keeps vortex 0.75's codec set
  (Pco, with ALP, ALPRD, integer Pco, Delta and OnPair excluded). The corpus goldens (`corpus/corpus.json`,
  `corpus/files`, `corpus/ingest-corpus.json`, the `logical_tables` walkthrough's `scan.tsra`) are regenerated.
  ADR-0024 records the event. The sink of `table::encode_streaming_to` and `TableStreamWriter::finish_to` must now
  be `Send`, because vortex's writer requires it (#635).

- **Breaking for writer output, `tessera-io` / `tessera-ingest`:** a table above `BLOCK_ROWS` rows is written as the
  leaves `<table>/0000`, `<table>/0001`, … (`events/0000` for GE listmode), each carrying the partition marker
  `{"index": i, "last": b}`, where it was `<table>_0000`, … without one (ADR-0064 PR D, #712). This covers GE
  whole-file and streaming ingest and `tessera bench --input` (`TableMultiBlockSink`). The payloads, block digests
  and so `content_hash` are unchanged for the same rows; the names and markers move `manifest_hash`. A table of at
  most `BLOCK_ROWS` rows stays one bare-named block, byte-identical. Products written before still read unchanged.
  `table::block_name` now returns `Result<String>` (the leaf name is checked against the block-name grammar), and
  the new `table::block_partition` gives each block's marker.
- **Breaking for struct literals (pre-1.0), `tessera-core`:** `TableSpec` has a new public field,
  `partition: Option<Partition>` (ADR-0064, #728). A `TableSpec { .. }` literal outside this workspace no longer
  compiles until it adds `partition: None`; downstream crates such as visionx-rs need that one-line change.
  Serialized specs are unchanged, because `None` is skipped.
- `tessera-io`: `Reader::logical_table` reports a name it can't resolve, or blocks that disagree on their schema, as
  `Error::LogicalTable` with a typed `LogicalTableError` reason, where it returned `Error::Container` messages; a
  caller matching `Error::Container` for "no blocks for prefix" must match `LogicalTableError::NotFound`. A single
  non-table block (an array) is now `NotATable`, and a legacy `events_NNNN` block whose spec doesn't parse as a
  `TableSpec` is `SchemaMismatch` (it was `Error::Serde`). `Reader::block_group` is deprecated in favour of
  `table_group`; its behaviour (the legacy flat rule only) is unchanged.
- `tessera-io`: `LogicalTableView::prefix` is deprecated in favour of `LogicalTableView::name`. The view is resolved
  from a table name, which is no longer always a block-name prefix; both return the same string.
- `tessera-io`: `unpack` and `pack_dir` refuse a product with an invalid partitioned table (`Error::LogicalTable`,
  ADR-0064 §2.4) before writing anything, as `ProductBuilder::seal` and so `WriteSession::seal` already do.
- **Breaking for `table_job_streaming` above `BLOCK_ROWS` rows, `tessera-io`:** the job fails with
  `Error::StreamedTable` (`StreamedTableError::NeedsPartition`, naming `StreamWriter::push_table`) where it wrote one
  block of any size; the alpha.4 "any size" contract is withdrawn (ADR-0064 §5.4). At or below `BLOCK_ROWS` it is
  unchanged. It is deprecated in favour of `push_table`.

### Removed

### Fixed

- `tessera-core`: a partition marker whose forged index makes the canonical leaf name longer than 255 bytes is
  refused as `LogicalTableError::NonCanonicalLeaf`, not a bare `Error::InvalidBlockName` (#733).
- `tessera-core`: `table_group` refuses a marked legacy-named block under a node (`a/pairs_0001`) with the error the
  seal gives for its parent node, instead of resolving a legacy flat group around it (#733, ADR-0064 §3 rule 4
  clarified).
- `tessera-io`: a `WriteSession` holds each committed block until the seal as its journal line, the compact JSON of
  its manifest entry, instead of a cloned `BlockRef` whose spec is a `serde_json::Value` tree. Per leaf of a
  12-column table the session held 13.5 KB of heap in small allocations for a 2.4 KB entry (12.8 KB live for the
  #715 consumer's 3.1 KB entry); it now holds the entry's bytes in one allocation, plus the leaf's name.
  `tests/session_leaf_memory.rs` checks the bound. Sealed bytes are unchanged; no public API changes (#759).
- CI: the flake checks that run tessera's tests through cargo (`workspace-test`, `workspace-doctest`, `sql-tests`,
  `s3-range-read`, `seal-profile-determinism`, `ingest-gate-a*`) put `TMPDIR` on a RAM fs (`/dev/shm`) on Linux. Each
  `WriteSession` commit fsyncs three times (#695), and on a ksb pod's shared `/nix` volume two tessera-io tests (0.97 s
  and 0.07 s on tmpfs) were still running after 3,240 s and 840 s, timing out the x86_64 leg. The tests check logic and
  fsync order, not disk speed; one journal fsync per batch is #798. A check whose `TMPDIR` is not on tmpfs or ramfs, or
  has less than 256 MiB free, fails before its tests run; the directory is removed whether the check passes or fails; a
  check that runs cargo tests without the helper fails evaluation; and `ram-tmpdir-guard` tests the helper's assertions
  and hook order (#793).

### Security

- `tessera-core`, `tessera-io`, `tessera-ingest`, `tessera-py`, `tessera-cli`: a sealed JSON number is an integer within
  ±(2^53−1) or it is refused (I-JSON, RFC 7493 §2.2; SPEC §1; ADR-0020 amendment; #886). JCS writes every number as a
  double, so a manifest sealed with `9007199254740993` verified when served with `9007199254740992`: the seal did not
  pin the value. The new `Error::InexactInteger` names the value's path (`metadata.daq_start_ns = 1730000000000000000
  exceeds ±(2^53−1) …; write it as a string`). `canonical::parse_strict` refuses it in every manifest, collection,
  embedded schema and signature envelope a reader parses (the envelope is now parsed through it too, so a duplicate
  key there is refused as well), and `canonical::to_bytes` refuses it before anything is hashed, so no writer seals
  one, a `u64` from Rust code included (new `canonical::check_i_json`, `canonical::MAX_EXACT_INTEGER`); a streamed
  `WriteSession` refuses it when it is declared (`append_block*`, `with_field`, `with_extra`, `with_generation`), not
  at the seal. The envelope is parsed by the new `Signature::from_json`, which `tessera-wasm` uses too. Python
  `Builder.set_field` and `with_generation`, a TOML spec's `[product.metadata]` (`spec::validate`, before anything is
  written) and CLI `commit --set` refuse it with that advice and never convert it; `with_generation` now names the
  rule for any int beyond the bound instead of `does not fit in 64 bits`. Products sealed earlier that hold such a
  number are refused (fail closed); `tessera provenance` reports such a parent as CORRUPT, never as missing, naming it
  by the `id` it claims (new `tessera_io::read_claimed_id`). No corpus seal moved, and no ingest backend writes such a
  number (DICOM values are strings). The reference reader refuses the same numbers, and `corpus/seal-cases.json` adds the cases.
- `tessera-core`: `Manifest::from_json_verified` checks `manifest_hash` over the manifest JSON object as parsed (SPEC
  §4), not over a re-serialisation of the `Manifest` struct. Before, a key spliced into a sealed `manifest.json`
  (`"forged": true`) was dropped by the struct and `tessera verify` reported OK, while a product a newer writer
  sealed with an extra field, `"study": null` or no `sources` key was reported as tampering. A manifest read from
  bytes must now carry `content_hash` and `manifest_hash`: before, deleting the seal (or setting it to `null`) skipped
  the check, so any edit verified. Both readers (Rust and `corpus/reference_reader`) refuse a manifest that is not
  I-JSON (a key repeated in one object, `NaN`, a lone surrogate), and reach the same verdict on every case of the new
  `corpus/seal-cases.json`. Every corpus seal is unchanged (#573, ADR-0060 §2.2 P0a).
- `tessera-core`, `tessera-io`: block names are unique within a product, and a manifest that lists one name twice is
  refused with the new `Error::DuplicateBlockName` (#734, ADR-0063 amendment, SPEC §5 and §7). Before, a manifest
  listing `data` twice, the second entry carrying the digest of bytes the archive doesn't hold, verified (`OK`, 2
  blocks) once re-sealed: a name resolved to its first listed digest, and the member check compared the listed names
  as a set, so the second digest covered nothing. `Manifest::from_json_verified` and `Manifest::verify`, and so
  `Reader::open`, every read, `verify`, the repository and `pack_dir`, refuse the repeat before any digest lookup (the
  deep verify's member check also runs it, as defence in depth). Every writer refuses to make one:
  `ProductBuilder::seal` (and so a write session's seal, the `StreamWriter`, `tessera commit --add-block`, ingest and
  the Python `Builder`), a write session's append, `pack` / `pack_streaming`, and the repository's `commit` and
  `put_manifest`, which stored the payloads and moved the lineage tip to a manifest `get_manifest` refuses. The append
  and `commit --add-block` refused a repeat before with `Error::Invalid`; they now return `Error::DuplicateBlockName`
  like every other door. Names compare by exact bytes: `Data` and `data` stay distinct (case folding is #690). The
  reference reader refuses the repeat by name too, and `corpus/seal-cases.json` gains the case
  `recon_int16/block-listed-twice` (`refused`) for both readers. No valid product changes, and no corpus
  `manifest_hash` moves.
- `tessera-core`: `Collection::from_json_verified` (and so `tessera collection verify`) checks a collection's
  `manifest_hash` over the `collection.json` object as parsed, the rule #573 set for manifests. Before, a key spliced
  into a sealed collection (`"forged": true`, at the top level, in `level` or in a member) verified OK, a collection
  a newer writer sealed with an extra field was reported as tampering, and a deleted seal skipped the check. A
  collection read from bytes must now carry `content_hash` and `manifest_hash`, and a key repeated in any object is
  refused. Fields from a newer format revision are named by the new `Collection::newer_fields()` and warned about by
  `tessera collection inspect`/`verify`, whose seal badge is now computed over the parsed object too (#873, ADR-0060
  §2.2 P0b).
- `tessera-core`: `ProductSchema::from_value`, and so `validate_manifest`, refuse an embedded schema carrying a key
  this build doesn't model (at the top level, in `fields[]`, `blocks[]`, `tables.<role>` with its columns and
  references, or `arrays.<role>`) with `Error::NewerFormatFields`, naming its path. Before, the key was dropped and the
  product was reported schema-valid against a rule that was never evaluated. Known keys holding their defaults
  (`inherit: false`, `requires_generation: false`) still validate. The new `ProductSchema::view_of` returns the typed
  view plus those paths for display: `tessera schema` shows the roster, warns and then fails validation; `ls FILE
  schema` names the rules; the `tree`/`inspect` badge reads `schema=<p>?(newer rules, not validated)`; `schema
  --json` prints the embedded object as stored instead of its re-serialised view (#873).
- `tessera-core`: the manifest, a collection and an embedded product schema are read in their declared spelling only
  (SPEC §4, §5): records as JSON objects, enumerated names as strings. serde also built a record from a JSON array,
  positionally, so a record spelled `["t","u8",…]` verified and validated as `{"name":"t","dtype":"u8",…}`, a second
  spelling of one sealed value, and the keys inside it (a column's `min`, a reference's `to.strict`, a
  `referencing.transform.x`) were never named as newer fields, so a rule a newer writer added could be dropped unseen.
  It also read a name from a one-key object (`"kind": {"array": null}`). `Manifest::from_json`/`from_json_verified`
  and `Collection::from_json`/`from_json_verified` now refuse either spelling anywhere (the whole object, `producer`,
  `generation`, `blocks[i]` and its `kind`, `sources[i]`, `ingest_transform[i]`, `level`, `members[i]` and its
  `role`/`kind`) with `Error::Serde` naming its path; `ProductSchema::from_value`/`view_of` (so `validate_manifest`)
  refuse it in any schema record, an internally tagged `referencing.transform` included, with `Error::Invalid`. The
  reference reader, which reads manifests only, refuses the same in a manifest, and `corpus/seal-cases.json` gains four
  `refused` cases for both readers. No product a tessera writer sealed changes, and no corpus `manifest_hash` moves
  (#873).
- `tessera-core`: the spelling check also covers a key the typed view drops as a default (`skip_serializing_if`).
  Before, it compared only the keys the view wrote back, so a default unit variant spelled as an object (a collection
  member's `"kind": {"product": null}`, a schema table's `"order": {"sequence_key": null}`, a contract column's
  `"sensitivity": {"public": null}`) verified or validated under a seal of its own: a second sealed spelling of one
  collection, with the same `content_hash`. Such a key's value must now be its default's own spelling (`null`, a
  boolean, a number, a string, `[]` or `{}`), and anything else is refused like the forms above. The explicit default
  itself (`"study": null`, `"kind": "product"`, `"sequence_key": []`) is still accepted, as SPEC §4 states (#873).
- `tessera-core`: a newer field is decided by where the key sits (its record doesn't declare it), never by a path
  string. The spelling check exempted a key whose dotted path was in the newer-field list, but a key's name can
  spell another key's path: a top-level key named `"members[0].kind"` exempted the first member's real `kind`, so
  `"kind": {"product": null}` verified (and `tessera collection verify` passed it with a warning). The walk and the
  check now share structured paths, and such a collection, or an embedded schema with a crafted
  `"tables.events.order"`, is refused. Newer-field paths are rendered unambiguously wherever they are reported
  (`Manifest::newer_fields`, `Collection::newer_fields`, `ProductSchema::view_of`, `Error::NewerFormatFields`,
  the CLI warnings): a key that is not a non-empty run of ASCII letters, digits and `_` starting with a letter or `_` is quoted, `["producer.agents"]`,
  so it never reads as the nested `producer.agents` (SPEC §4). Every path a well-formed product declares reads
  as before. `corpus/seal-cases.json` gains the case `recon_int16/key-spelling-a-nested-path-resealed` (#873).
- Reference reader (`corpus/reference_reader`): it checks every declared manifest field's JSON type and shape at every
  depth from one declarative table that mirrors the Rust types (strings, booleans, block-kind names, the block-name
  grammar, string-valued `id_inputs`, objects for the opaque maps, required fields), refuses a block without a
  digest, parses a `tessera_version` major as the Rust reader does, and refuses nesting deeper than 127 levels, an
  integer out of a double's range, and a manifest that is not UTF-8 (UTF-16, a byte-order mark).
  Before, `metadata: []`, `study: 5`, `blocks[0].kind: "hologram"` or `producer.dirty: 1` verified there while the
  Rust reader refused them, `id_inputs: []` was `tampered`, and `blocks[0].name: {}` or a missing `digest` raised
  instead of giving a verdict. Every input now gets a verdict, an archive `zipfile` can't read included.
  `corpus/seal-cases.json` gains 19 cases for both readers, and `run.py` five crafted archives (#873).
