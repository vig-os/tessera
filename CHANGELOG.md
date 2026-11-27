# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## Unreleased

### Added

- `ObjectKind::of` classifies version-object bytes without guessing (ADR-0067 step 3, #998).
  Ambiguous shapes are refused. `collection::diff` reports added, removed and repinned members in
  collection order, detects relative order changes, and refuses duplicate member references.

- Collections carry sealed `sources` edges with `supersedes` and `forked_from` roles (ADR-0067 step 2, #997).
  Version edges reference a predecessor's id and pin its manifest seal, independently of the member MMR.
  `CollectionBuilder::add_source` records edges; `with_id_inputs` accepts caller-declared identity, including
  the required non-empty `fork` token for a forked lineage. Both version roles require a predecessor seal
  pin. One writer validation rule refuses missing/empty tokens or pins with `Error::Invalid` when sealing,
  recomputing a seal or emitting canonical/stored bytes, including after public-field mutations. Readers
  still verify received bytes. Empty sources are omitted, preserving existing collection bytes. Native, reference and wasm readers share seal vectors.

### Changed

- **breaking (writers)**: independent tables sharing a legacy numbered prefix, such as `run_0001`
  and `run_0002`, must agree on column count, names, order, dtypes and nullability to seal (#978).
  Rename independent tables without a shared `_NNNN` prefix, for example `run_first` and `run_second`.
  Existing products retain the legacy read rule; incompatible group reads remain refused.

- **breaking**: repositories now require layout 2 (`tessera-repository 2`), with typed product ref records
  in `refs/lineages/` and `refs/pins/` and lineage caches in `log/lineages/` (ADR-0067 step 1, #996).
  Every other layout, including an unmarked empty directory, is refused. There is no migration verb: seal tips
  with the build that wrote the old repository, then import into a new one. Both namespaces share injective
  percent-encoding and a 249-byte name bound; `InvalidRefName` replaces `InvalidLineage`. Garbage collection
  roots include both namespaces and corrupt records or stray entries abort it before any deletion.

### Removed

### Fixed

- **Collection member filenames are injective (#994).** Members and repository refs share the percent encoder
  from #975 (`a:b` → `a%3ab`, `a_b` → `a_b`, `Ab` → `%41b`). Ref filenames are unchanged. Local collections,
  ingest output and S3 prefix exports use the same encoded stems, including nested descriptors and seals.
  Older local underscore layouts and raw-id prefix exports must be recreated from source products; verification
  diagnoses a legacy layout only when an old-named file exists, without reading legacy aliases. Missing-member errors
  report the encoded filename; `collection ls` marks absent members and continues listing.
  OCI artifacts remain digest-addressed and need no filename migration.

- Refuse partitioned and legacy flat tables whose leaves disagree on column names, dtypes or
  nullability before sealing or packing (#978). Readers continue to refuse incompatible sealed tables.

- **The sealed `ingest_decoder` names the decoder the binary links (#979).** Its `version` and its `features`
  pre-image were read from `Cargo.lock` by `tessera-ingest`'s build script, whose output cargo reuses across two
  checkouts that share a target dir. A binary could link parquet 58.4.1 and seal `=58.4.2`. The library now takes
  the resolution with `include_str!` in its own compile unit, which cargo rebuilds whenever a linked crate
  changes. `tessera info` reads the same values. No sealed value moves on a clean build, so the ingest corpus is
  unchanged (ADR-0056 §6a, amendment #979). `Decoder`'s `version` and `pins` fields are replaced by the
  `Decoder::version()` method and the existing `Decoder::feature_preimage()`.
- **Distinct lineages no longer share a ref file (#975).** Ref and log cache names were made by replacing `:`, `/`
  and `\` with `_`, so `a:b` and `a_b` shared one tip: a commit on one replaced the other's tip, or since #946 was
  refused with `RefMoved`. Names are now percent-encoded (`blake3:3f…` → `refs/lineages/blake3%3a3f…`), injective on
  case-insensitive file systems too (ADR-0036, 2026-11-25 amendment).

### Security

## [0.1.0-alpha.7] - 2026-11-25

### Format break

- **This release breaks the on-disk format (ADR-0066 option A′, #914, PR #940).** The seal (`manifest_hash`) is
  `blake3` over the stored bytes of `manifest.json`, which is stored as exactly its RFC 8785 JCS form, with no
  `manifest_hash` key. The seal is its own member, `manifest.seal`, the second zip entry of a `.tsra` (after
  `mimetype`, before `manifest.json`; SPEC §6). The details are under Changed.
- **Products written by v0.1.0-alpha.6 or earlier are refused, and there is no migration verb (ADR-0066 rule 10).**
  The error names the layout it found and the layout it requires. Re-seal from source: ingest or write the product
  again with this release.
- **Which hashes moved, and why:**
  [`docs/spikes/916-format-break-crosscheck.md`](https://github.com/vig-os/tessera/blob/dev/docs/spikes/916-format-break-crosscheck.md)
  (#916, PR #980) traces every move across the 22 corpus products. The A′ cutover itself keeps the value of every
  seal (PR #940). Derived blocks leave `content_hash` (#720, PR #942), so `content_hash`, and with it
  `manifest_hash` and every signature over it, moves for a product that carries a derived block (a `<name>.cidx`
  chunk index, a `tessera pyramid` level above 0); no corpus product carries one. `manifest_hash` moved on 10 of
  the 22: through #895 (PR #934) on the 3 that embed the built-in
  `recon` schema, now version 1.1 with number ranges, and through PR #972 on the 7 Arrow-backed ingest fixtures,
  whose sealed decoder version moved with the parquet/arrow 58.4.2 bump (RUSTSEC-2026-0595, recorded per ADR-0056
  §6a). No `id` and no block digest moved.
- **Signatures made before the break no longer verify where the seal moved (ADR-0066, Consequences), and the
  alpha-layout product a signature was made over is refused.** A signature over a seal that did not move (12 of the
  22 corpus products) still verifies against the re-sealed product: the signature envelope (`ed25519-env-v1`) is
  unchanged (ADR-0066 rule 8). Sign again every product whose seal moved.
- **Not in this release: #828,** the `tessera-table-v2` per-column codec profile (ADR-0066 step 7, deferred by the
  owner on 2026-11-22). `tessera-table-v1` stays the table codec profile, so table block digests do not move.

### Added

- **Breaking for `FieldSpec` struct literals, `tessera-core`:** a product schema's metadata field declares
  `encoding` (`Some(Encoding::String)`, a 64-bit integer sealed as a decimal string) and an inclusive `minimum` /
  `maximum` (`Option<f64>`, within ±(2^53−1)), with the builders `string_encoded`, `minimum`, `maximum` and `range`
  (ADR-0020 and ADR-0060 2026-11-19 amendments, #895). `FieldSpec` isn't `#[non_exhaustive]`, so a struct literal
  outside tessera stops compiling until it names the three fields; build them with `None`. Unset, the keys are
  omitted, so a schema that declares none of them keeps its bytes. New `ProductSchema::check_definition`,
  `check_value` and `check_metadata`, `schema::declared_schema`, and the typed `Error::SchemaField` and
  `Error::FieldValue`, which name the field and the value's source (a DICOM tag and its text, a spec key, a CLI flag).
  The built-in `recon` 1.1 ranges `kvp` to 0–1000 kV and holds `slice_thickness` to at least 0 mm; `sim` 1.1
  declares `seed` a string-encoded `int64`. This moves the seal of the three `recon` corpus products.
- `tessera-ingest`: `dicom::read_object_with` and `dicom::to_recon_product_with` read a DICOM object and build its
  `recon` product against a given `recon` schema, so the curated numbers' spelling and ranges come from that schema
  rather than the built-in one; `read_object` and `to_recon_product` use the built-in schema (#895).

### Changed

- **Breaking API (`tessera-io`, #946, ADR-0036 §2):** `Repository::commit(lineage, expected, manifest, payloads)`
  takes `expected: Option<&str>`, the tip the caller built the version on (`None` for a lineage's first version). A
  ref elsewhere is refused with the new typed `Error::RefMoved { lineage, expected, found }` (`tessera-core`) before
  anything is stored; with a parent, the manifest must carry exactly one `supersedes` edge, to it
  (`Error::SupersedesMismatch`). `set_ref` returns `Error::RefMoved` (was `Error::Invalid`) and refuses a new tip
  that is not a `blake3:<64 hex>` address. `tessera commit` passes the tip it read; `tessera import` into a lineage
  whose tip is another version is refused instead of replacing its tip (importing the tip's own version again
  moves nothing and rewrites its missing or torn objects).
- **Breaking format change (#720, ADR-0028 §4, ADR-0066 rule 12):** `content_hash` is the Merkle root over the
  **primary** blocks only. A block ref gains an optional `"class": "derived"`, set by the writers of accelerator
  blocks: the chunk-index sidecar `<name>.cidx` (`tessera_io::chunk_index::chunk_index_block`, so every `*_indexed`
  path and `array_block_with_index` / `table_block_with_index`) and the levels above 0 of `tessera pyramid`.
  `tessera commit --add-block` lands a copied block **primary**, whatever its class in the source (a derived block
  is derived of the product that computed it). It never copies a derived block: when the source indexes the copied
  block, `commit` rebuilds `<name>.cidx` from the copied bytes, with the writers' own `array_chunk_index` /
  `table_chunk_index` (`tessera_io::chunk_index::rebuild_chunk_index_block`), and lands that derived. A source's
  index is bound to its block by name only, so its bytes are never trusted: `--add-block` refuses to copy any chunk
  index (kind `chunk_index`, or a spec with `indexes`), whatever its name or class, and reads only from a source
  `tessera verify` accepts. `tessera stats` and `LogicalTableView` pruning take aggregates only from a derived index
  of the block (`block::trusted_chunk_index`: named `<name>.cidx`, kind `chunk_index`, derived, `indexes` =
  `<name>`); a primary look-alike is ignored and the data is read instead. A chunk index (kind `chunk_index`, or a
  spec with `indexes`) whose `indexes` names no other block of its product is a structural defect like a repeated
  name: every reader refuses it (`Error::DanglingChunkIndex`, naming the index and the missing block; Rust open and
  verify, `tessera-wasm`, the Python reference reader; new seal cases and a refusal case) and every writer refuses
  to produce it (`ProductBuilder::seal`, `pack`, `pack_streaming`, `Repository::put_manifest` / `commit`; shared
  check `block::check_block_refs`). So an orphan index can no longer be imported and inherited by a block added later.
  `tessera commit --remove-block` removes the block's derived family with it: its derived `.cidx` and, for a pyramid's `<base>/0`, its derived levels and their sidecars
  (`ProductBuilder::remove_block_with_derived`, `block::derived_family`, with the writers' naming rules
  `block::cidx_name` / `block::pyramid_level_name`); the commit summary counts the cascade, `-1 (+2 derived)`. A
  primary chunk index of the removed block is never removed implicitly: the remove is refused, naming the index and
  `--remove-block` for it, since it would dangle. A
  derived block is still sealed: `manifest_hash` covers it, its digest and its class, and its payload is checked
  against its digest like any block's. Adding, regenerating or dropping one moves `manifest_hash` only; a product
  whose blocks are all derived has the `content_hash` of the empty set. The key is never written for a primary
  block, so this change leaves products without derived blocks byte-identical (all 18 tracked `.tsra` files and
  every `corpus.json` golden are unchanged by it). **What breaks:** a product written with a `.cidx` or by
  `tessera pyramid` now has a
  different `content_hash` (and `manifest_hash`) than the same input gave before, and a reader from before this
  change can't verify it: it reports `blocks[n].class` as a newer field and finds `content_hash` tampered. Products
  from v0.1.0-alpha.6 or earlier are refused by the A′ layout check anyway (see Format break). Every reader
  (Rust, `tessera-py`, `tessera-wasm`, the Python reference reader) refuses a `class` other than `"derived"`
  (`"primary"` and `null` included). `tessera-core`: `BlockRef` gains the public field `class: BlockClass`
  (so struct literals must set it), with `BlockRef::derived()`, `BlockRef::is_primary()` and the shared fold
  `block::content_root`, which `ProductBuilder::seal`, `Manifest::recompute_content_hash` and the streaming
  `WriteSession` running root all use. The `.cidx` spec no longer carries `class` (it moved to the ref);
  `corpus/seal-cases.json` gains 14 derived-block cases (164 in all).
- `tessera-py` (**breaking return type**, #804): a nullable table column reads as an `np.ma.MaskedArray`, masked
  (`mask=True`) at each NULL, from `Reader.column`, `logical_column` and `table_dict`, whether or not its rows hold
  a NULL; a column that is not nullable is still a plain `np.ndarray`. `table_arrow` gives a nullable column a real
  Arrow validity bitmap (`null_count` counts the NULLs) and `table` native polars nulls; a present NaN stays a value.
  The native `read_table` entries are now `(name, le_bytes, numpy_code, validity)` and `read_table_column` /
  `read_logical_table_column` return `(le_bytes, numpy_code, validity)`: `le_bytes` holds the values only and
  `validity` is `None` for a column that is not nullable, else one byte per row (`1` present, `0` NULL).
  `tessera-io` adds `ColumnData::values_and_validity`, which takes a column apart into those two parts.
- **Breaking format change (ADR-0066 A′, #914, #901):** the seal is `blake3` over the **stored bytes** of
  `manifest.json`, which is stored as exactly its RFC 8785 JCS form with no `manifest_hash` key; the seal lives in a
  new `manifest.seal` member, entry 2 of the `.tsra` (after `mimetype`, before `manifest.json`), beside
  `manifest.json` in the exploded form, and in `collection.seal` beside a `collection.json`. Every reader (Rust,
  `tessera-py`, `tessera-wasm`, the Python reference reader) refuses bytes that are not their own JCS form, so a
  respelling the attacker seals (#901's `64.0` for an integer `64`, a pretty layout) no longer verifies or splits
  readers. **Products written before this change are refused**, with an error naming the layout found (the declared
  `tessera_version`, the missing `manifest.seal`, an in-band `manifest_hash`, a non-JCS `manifest.json`) and the
  layout required; there is no migration verb: re-ingest them. The A′ cutover on its own keeps every seal's value
  (the 13 tracked products re-pack with every `corpus.json` golden unchanged); across this release, seals moved
  where #895, #972 or a derived block changed the manifest (see Format break). `TESSERA_VERSION` stays `"0.0.0"`.
  Repository manifest objects hold the stored bytes, so `objects/<manifest_hash>` hashes to its own name.
- **Breaking, `tessera-core`:** `Manifest::manifest_hash` and `Collection::manifest_hash` are no longer serialised
  (the seal is not a field). Readers verify through `Manifest::from_stored_members` / `Collection::from_stored_members`
  (`from_stored` with the seal checked); writers store `to_stored()`. New errors `Error::UnsupportedLayout` (the alpha
  layout) and `Error::MalformedSeal` (a seal member that is not `blake3:<64 lower-case hex>`; its `member` is a
  `String`), and the `tessera_core::stored` module (member names, `parse_seal`, `seal_name_for`).
  `Collection::from_stored_members_as` reads a sub-collection stored as `<stem>.collection.json`, so its refusals name
  `<stem>.collection.seal`. A document without its seal member is called the alpha layout only when its bytes show
  it (an in-band `manifest_hash`, a non-JCS form) (#939).
- **Breaking, `tessera-io`:** `stored::manifest` / `stored::collection` return the `(bytes, seal)` pair;
  `stored::write_pair`, `read_pair`, `write_collection`, `read_collection`, `collection_from_pair` and `seal_path` are
  the one place documents and seals are written and read on disk. `write_pair` is crash-consistent: each file is
  written durably through a temp file and a rename, the seal last, so a crash leaves either the previous pair
  whole or a pair that a reader refuses (a seal mismatch, or no seal), never a torn file or a mixed pair that
  verifies (#939). `prefix_layout` lists the `collection.seal` objects.
- **Breaking, `tessera-wasm`:** `verify_manifest(manifest_json, manifest_seal)` and
  `verify_signature(manifest_json, manifest_seal, signature_json, pubkey_hex)` take the seal member; the new
  `seal_verdict` returns the `corpus/seal-cases.json` verdict, and the `wasm-bindgen-smoke` check runs every seal
  case through the compiled wasm in Node. The members, and `product_id`'s manifest, are the stored **bytes**
  (`Uint8Array`), no longer strings: a default `TextDecoder` drops a leading byte-order mark and maps invalid UTF-8
  to U+FFFD, so a string-taking verifier accepted manifests the native reader refuses. The strict UTF-8 parse is
  in Rust, and the seal is checked over exactly the bytes passed.
- The Python reference reader refuses an archive outside SPEC §6's member set, as the Rust reader does: a member
  name listed twice, or an entry that is not STORED (it read the last of two `manifest.seal` or `manifest.json`
  entries, and inflated a DEFLATED one). The committed `corpus/refusals` fixtures hold both readers to it.
- `tessera-py`: `Reader.manifest_hash` gives the seal; `manifest()` no longer has a `manifest_hash` key.
- `tessera inspect --json` prints the manifest's fields without `manifest_hash` (the summary's `manifest_hash` line
  shows the seal).
- **Breaking, `tessera-core`:** a product schema with a bare `int64` / `uint64` metadata field, one without
  `"encoding": "string"`, is refused at the seal (`Error::SchemaField`), as are an unknown dtype, an encoding or range
  on a field it doesn't fit, an empty range, an impossible `default` and a repeated field id (#895). A product
  embedding the old `sim` 1.0 schema can no longer be re-sealed or validated by this build, and a numeric `seed` is
  refused. Re-sealing keeps the rules of the schema the product embeds, so a product embedding `recon` 1.0 takes any
  `kvp`.
- **Breaking, `tessera-ingest`:** a curated DICOM numeric tag (`KVP`, `SliceThickness`, `PixelSpacing`) whose
  non-empty text is not a number is refused when the file is read, naming the tag and its text, where it was
  dropped without a word; an empty tag is still left out (#895).

### Removed

- `tessera-core`: `Manifest::from_json_verified`, `Manifest::to_sealed_json`, `manifest::seal_of`,
  `Collection::from_json_verified`, `Collection::to_sealed_json`, `Collection::from_json_with_seal`,
  `canonical::hash_without` and `canonical::bytes_without`: each verified or sealed a re-serialisation of a parsed
  object, the path ADR-0066 removes (one verify path, `from_stored`; #931 item 1). `tessera-io`:
  `stored::canonical_form` (re-spelling a document a door copies; doors now copy the stored bytes verbatim).

### Fixed

- `tessera-py`: a nullable table column no longer reads back with its bit-packed validity mask appended to its
  values (#804). The native layer handed Python `ColumnData::to_le_bytes`, which for a nullable column is the
  staging form `values ‖ ceil(rows/8)` mask bytes, so `np.frombuffer` either raised (an `i4`/`f8` column whose
  length the trailer broke) or silently returned extra elements made of mask bits (a `u2` column of 11 rows read as
  12), and every NULL read as `0` with no mask. The values and the mask now cross the boundary separately. The
  other consumers of `to_le_bytes` were not affected: the CLI `read`/export renders a nullable column from its
  `ColumnData` (NULL as JSON `null`, CSV `nan`), `tessera sql` builds an Arrow validity bitmap, the streaming
  staging fragments invert the trailer with `from_column_bytes` given the row count, and the chunk-index digest
  hashes the staging form by design; the WASM binding reads no table payloads.
- `tessera-io`: a repository is a fenced single writer (#946, ADR-0036 §2 amendment). Every write (`commit`,
  `set_ref`, `write_object`, `put_manifest`, `forget`, `gc`) holds one exclusive OS advisory lock (`flock`) on
  `<repo>/lock` from its check to its last write, so two overlapping commits can no longer both return Ok with one
  of them unreachable, and `gc` can no longer sweep a commit's objects before its ref advance. Objects and refs are
  written to `<repo>/tmp/`, fsync'd, renamed into place and their directory fsync'd, objects before the ref, so a
  crash leaves the old version or the new one. An object torn at its address is detected by hash and replaced by
  the next write of its bytes. An empty or unparsable ref is the new `Error::CorruptRef`, never "no commits yet".
  `gc` aborts with the new `Error::GcAborted`, naming what it could not read and deleting nothing, on a ref or
  manifest it cannot read (it treated an unreadable manifest as having nothing to keep, and deleted the history
  behind a refused tip), and walks a tip whose address another version stores as a block. The lock is valid on a
  local filesystem only, not on NFS. Cost: a write now reads back and hashes an object already at its address
  before it skips it, so re-importing a stored product reads its blocks once more; each commit adds the fsyncs of
  its new objects and its ref.

### Security

- Dependencies: `url` 2.5.8 → 2.5.9 for RUSTSEC-2026-0592 (`Url::origin` / `quirks::origin`; #953, PR #954).
  `Cargo.lock` only, plus `form_urlencoded` 1.2.3, which `url` 2.5.9 requires.
- Dependencies: `parquet` and the `arrow-*` crates 58.4.1 → 58.4.2 for RUSTSEC-2026-0595 (bloom filters written as
  plaintext into encrypted Parquet; #970, PR #972). tessera writes no encrypted Parquet and sets no bloom filters,
  so it cannot reach the new refusal. The sealed ingest decoder version moves with the bump, so the `manifest_hash`
  of the 7 Arrow-backed ingest fixtures moves (ADR-0056 §6a).

## [0.1.0-alpha.6] - 2026-11-19

### Added

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

### Fixed

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
  written) and CLI `commit --set` / `ingest … --meta` refuse it with that advice and never convert it; `with_generation`
  now names the rule for any int beyond the bound instead of `does not fit in 64 bits`. The CLI reads a typed value
  through the new `canonical::parse_user_value`: text that is JSON by the RFC 8259 grammar stays JSON, so
  `--set x=1e400`, a 401-digit integer or `[1e400]` is refused (`--set x = 1e400 exceeds ±(2^53−1) …`, naming the flag
  and the typed text) instead of being sealed as the string it used to fall back to; other text is still a string. A DICOM
  curated number (`KVP`, `SliceThickness`, `PixelSpacing`, `RescaleSlope`/`RescaleIntercept`, the series'
  `ImagePositionPatient`/`ImageOrientationPatient`) stays a JSON number, and one beyond the bound is refused when the
  file is read, naming the tag and its text (`dicom: (0018,0060) KVP = "1E16" is outside the range a sealed JSON
  number can hold …`), before any output exists (new `canonical::beyond_exact_f64`). Products sealed earlier that hold such a
  number are refused (fail closed); `tessera provenance` reports such a parent as CORRUPT, never as missing, naming it
  by the `id` it claims (new `tessera_io::read_claimed_id`). No corpus seal moved; the DICOM header dump is text, and a
  64-bit DICOM value (`SV`/`UV`) is written as a string by its type. The reference reader refuses the same numbers, and `corpus/seal-cases.json` adds the cases.
- `tessera-core`, `tessera-io`, `tessera-ingest`, `tessera-cli`: `NaN`, `inf` and `-inf` are refused wherever a writer
  reads one, and a typed number must be spelled as JSON spells it (#886). `serde_json` writes a non-finite double as
  `null`, so a TOML `[product.metadata] x = inf`, a NIfTI `scl_slope` of `inf` or `NaN` and a typed value sealed
  `null` and verified with rc=0. Every f64→JSON conversion on a write path now goes through the new
  `canonical::json_number` (one value) or `canonical::to_json_value` (a typed document: block specs, manifests,
  collections, `generation`), which refuse NaN and ±infinity with the new `Error::NonFiniteNumber` (and a value beyond
  ±(2^53−1) with `InexactInteger`); a TOML `inf`/`nan` is refused by `spec::parse_str`, naming its path
  (`product[0].metadata.x = inf …`). NIfTI refuses `scl_slope`/`scl_inter` that are non-finite or beyond the bound at the
  door, naming the header field and its value (`nifti: scl_slope = "inf" is not a finite number …`; before, `1e16`
  was refused only by the generic message advising a string), and a DICOM `DS` of `NaN`, `inf` or `-inf` is refused by
  tag as `1E16` already was; both use one formatter. A `--set`/`--meta` text that Rust's `str::parse::<f64>()` reads
  as a number but JSON does not spell as one (`+5`, `.5`, `01`, `inf`, `Infinity`, `NaN`, `+1e400`,
  `+9007199254740993`) is refused, naming the flag and key and why, instead of being stored as a string; the
  JSON-quoted form (`--set k='"+5"'`) stores the text. `hello`, `0x10` and `1_000` are still strings, and `5` and
  `9007199254740991` still numbers. `01`, `.5` and `+1`, which were strings, are now refused: quote them to keep the text.
- `tessera-core`, `tessera-io`, `tessera-ingest`, `tessera-cli`: every NIfTI header value that feeds the manifest is now
  validated at the door, and the generic seal-time error stops giving advice a typed field cannot follow (#886). Round 2
  checked only `scl_*`; the sform `srow_x/y/z`, the qform `quatern_b/c/d`, `qoffset_x/y/z` and `pixdim[0..=3]`, the time
  step `pixdim[4]` and `toffset` still reached the seal-time check, which names a JSON path and said "write it as a string".
  One table-driven validation now refuses NaN, ±infinity and a value beyond ±(2^53−1) naming the header field and its value
  (`nifti: pixdim[1] = "10000000000000000" …`), and an affine entry computed from in-range fields that falls out of range
  is refused naming the fields it came from. A NaN `pixdim` was clamped to 1.0 and sealed with rc=0; it is refused (a
  zero or negative spacing is still clamped, as the reference `nifti_quatern_to_mat44` does). `Error::InexactInteger`
  gains `as_string`: only a free-form `metadata` field is told to write a string, a typed field (`column_meta.scale`, an
  affine, a transform) gets a neutral "outside the range a sealed JSON number can hold", and the non-finite error names its
  JSON path (`blocks[0].spec.scale`) instead of "a sealed document". A non-zero number that underflows to `0.0` (`5e-400`)
  sealed as `0.0` with rc=0 and is now refused in `--meta`, JSON values, TOML floats, a `--column-meta` `scale` and a
  DICOM `DS`; `0`, `0e5` and `1e-320` stay. `--meta k=1e400` is worded like `+1e400` ("past a double's range"), not "can't hold
  this integer exactly". `--column-meta` `scale` of `nan`, `inf` or `1e16` is refused naming the flag and the column.
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

## [0.1.0-alpha.5] - 2026-11-11

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

### Changed

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

## [0.1.0-alpha.4] - 2026-10-29

This file recorded no entries for this release. Its changes are listed per crate, in
`tessera/crates/<crate>/CHANGELOG.md`.

## [0.1.0-alpha.3] - 2026-10-19

This file recorded no entries for this release. Its changes are listed per crate, in
`tessera/crates/<crate>/CHANGELOG.md`.

## [0.1.0-alpha.2] - 2026-10-19

This file recorded no entries for this release. Its changes are listed per crate, in
`tessera/crates/<crate>/CHANGELOG.md`. The tag's macOS builds failed (#647), so no GitHub Release was published
for it; `v0.1.0-alpha.3` superseded it the same day.

## [0.1.0-alpha.1] - 2026-09-24

This file recorded no entries for this release. Its changes are listed per crate, in
`tessera/crates/<crate>/CHANGELOG.md`.

[0.1.0-alpha.7]: https://github.com/vig-os/tessera/compare/v0.1.0-alpha.6...v0.1.0-alpha.7
[0.1.0-alpha.6]: https://github.com/vig-os/tessera/compare/v0.1.0-alpha.5...v0.1.0-alpha.6
[0.1.0-alpha.5]: https://github.com/vig-os/tessera/compare/v0.1.0-alpha.4...v0.1.0-alpha.5
[0.1.0-alpha.4]: https://github.com/vig-os/tessera/compare/v0.1.0-alpha.3...v0.1.0-alpha.4
[0.1.0-alpha.3]: https://github.com/vig-os/tessera/compare/v0.1.0-alpha.2...v0.1.0-alpha.3
[0.1.0-alpha.2]: https://github.com/vig-os/tessera/compare/v0.1.0-alpha.1...v0.1.0-alpha.2
[0.1.0-alpha.1]: https://github.com/vig-os/tessera/releases/tag/v0.1.0-alpha.1
