# Tessera format specification (v0, pre-1.0)

Language-neutral. An independent implementation built from this document alone must reproduce the
golden hashes in `tessera/corpus/corpus.json` (the v1.0 conformance gate). Normative keywords:
**MUST** / **SHOULD** / **MAY**. Authoritative decisions: `docs/adr/0020`, `docs/adr/0022`.

## 1. Overview
A Tessera **product** is one immutable, content-hashed, self-describing FAIR data artifact: a JSON
**manifest** + zero or more shape-dispatched storage **blocks** (N-D arrays, columnar tables). The
canonical serialized form is a single sealed **`.tsra`** container.

**Every sealed JSON document is I-JSON** (RFC 7493), the input RFC 8785 (JCS, §2) requires (RFC 8785
§3.1): the manifest with its embedded schema, metadata, `generation` and `ingest_transform` params, the
`id_inputs` object, a collection, and a signature envelope. A writer MUST NOT seal, and a reader MUST
refuse, a document with a repeated key in any object, a lone surrogate, `NaN`/`Infinity`, or a **number
whose value is an integer beyond ±(2^53−1)** (9007199254740991, RFC 7493 §2.2). JCS serialises every
number as an IEEE-754 double (RFC 8785 §3.2.2.3), so two such integers can share their canonical bytes
(`9007199254740993` and `9007199254740992` both canonicalise to `9007199254740992`) and one seal would
verify either. The rule is on the value: `9007199254740993`, `9007199254740993.0`,
`9.007199254740993e15`, `-9007199254740993` and `1e300` are refused, and so is a token whose fraction the
double has already lost (`9007199254740992.5` reads as `9007199254740992`); `9007199254740991`, `-0` and
`1.5` are not. A reader parses each token to the nearest double, correctly rounded, so readers agree at the
bound (`9007199254740991.5` rounds to 2^53 and is refused; `9007199254740991.4999999999999999999999` is
not). Every double beyond 2^53−1 is an integer, so a large float written in scientific notation
(`6.022e23`) is refused as well. A larger integer (an epoch-nanosecond timestamp, a 64-bit serial or tick counter) is
carried as a decimal **string**; a writer refuses it with that advice rather than convert it silently, and
a typed source field that is 64-bit (a DICOM `SV`/`UV` value) is written as a string by its type, never
by its value. A source field that is a number (a DICOM `DS`/`FD` value such as `KVP`) stays a number and is
refused beyond the bound, naming the source field; a value a person types as a JSON number stays a number too. Array and table payloads are hashed as raw bytes and are unaffected. Products sealed before
this rule (≤ 0.1.0-alpha.5) that hold such a number are refused: their seal never pinned the value (#886).

## 2. Canonical encoding (hashing)
All hashing is over **RFC 8785 JSON Canonicalization Scheme (JCS)** bytes: object keys sorted by
UTF-16 code unit, no insignificant whitespace, ECMAScript-canonical number forms, UTF-8 output. A
conformant implementation MUST use a JCS encoder. Non-hashed serializations (e.g. the pretty
`manifest.json` stored in the container) MAY be any valid JSON — verification re-canonicalizes.
**Number forms** are the classic pitfall: `1`, `1.0`, and `1e0` MUST all canonicalize to `1` (and
e.g. `-1024.0` → `-1024`); implementations SHOULD use a tested JCS library, not a hand-rolled JSON
serializer, for the hashed form. Non-ASCII strings follow RFC 8785's escaping rules verbatim.

## 3. Hashes
- Algorithm: **BLAKE3**, 256-bit, output as lowercase hex with the prefix `blake3:`.
  `digest(bytes) = "blake3:" + hex(blake3(bytes))`.
- **Merkle root** over an *ordered* list of digest strings `d[0..n]` is the **Merkle Mountain Range
  (MMR)** root, with **domain-separated** leaves and interior nodes:
  - `leaf(d) = blake3(0x00 ++ utf8(d))` — one prefix byte `0x00`, then the **full digest string** `d`'s
    UTF-8 bytes. Note `d` is the prefixed string `"blake3:<hex>"` *as text* (e.g. `"blake3:9f86d0…"`),
    **not** the raw 32 decoded hash bytes — hash the string verbatim.
  - `node(l, r) = blake3(0x01 ++ l ++ r)` — prefix byte `0x01`, then the two 32-byte child hashes
    (raw `blake3` output bytes, not hex).
  - Build: fold each leaf into a list of **peaks** (roots of complete perfect subtrees). To append a
    leaf, make a height-0 node; while the last peak has the *same* height, pop it (the left sibling) and
    replace with `node(left, new)` at height+1. The **root** "bags" the peaks right-to-left:
    `acc = peaks[-1]`, then `acc = node(p, acc)` for each remaining peak `p` from right to left.
  - A single leaf returns its `leaf(d)`; the empty list returns `blake3("")` (blake3 of zero input bytes).
  - Output is `"blake3:" + hex(root)`. (This is ADR-0028's recursive MMR root; it supersedes the earlier
    flat `blake3(d[0] ++ … ++ d[n-1])` construction — a v0.2 identity revision.)

## 4. Identity model (three hashes)
- **`id`** (logical identity): `id = digest(JCS(id_inputs))`, where `id_inputs` is a JSON object of
  string→string, hashed verbatim under JCS. The keys `product`, `name`, `timestamp` are the writer's
  recommended default — **not** a reader-enforced requirement; any string→string mapping is valid.
  Stable across rename / byte re-encode.
- **`content_hash`** (data fingerprint): `merkle_root([block.digest for block in blocks])`, blocks in
  manifest order. Each block's `digest` is `digest(<the block's stored payload bytes>)`.
- **`manifest_hash`** (the seal): `digest(JCS(manifest_without_manifest_hash))` — the manifest object
  with **exactly one key deleted** (`manifest_hash`); no other normalization, key insertion, or
  empty-stripping is applied beyond JCS. All other fields, including `content_hash`, stay present. It
  transitively commits to every block digest and all metadata.
  - A reader computes it over the object **as parsed from `manifest.json`**, never over a re-serialisation
    of its own typed model: keys it doesn't model, an explicit `null` (`"study": null`) and an omitted
    default (no `sources` key) are hashed exactly as received. So a key a newer writer added verifies, and a
    key spliced into a sealed manifest does not. A product's manifest MUST carry `content_hash` and
    `manifest_hash`; a reader treats a missing one as an integrity failure, since deleting the seal would
    otherwise launder any edit.
  - `manifest.json` MUST be I-JSON (RFC 7493; JCS requires it, RFC 8785 §3.1): no key repeated within any
    object, at any depth (keys compare after unescaping), no lone surrogate, no `NaN`/`Infinity`, no number
    outside a double's range and no integer beyond ±(2^53−1) (§1), and nested at most 127 objects and arrays
    deep (the manifest object itself is level 1; RFC 8259 §9 lets a parser limit nesting, and this limit is
    fixed so every reader refuses the same text). The same holds for a collection. A reader MUST refuse anything else rather than keep the first or
    last of two values, because parsers differ on which one wins.
  - **Forward reading (RFC §13).** A key no field of this section declares is a field from a newer format
    revision: a reader verifies it as part of the seal, SHOULD warn that it is not interpreted, and reads
    the rest. A writer MUST NOT build a new seal from such a manifest (a new version, a re-seal), since it
    can't carry what it doesn't understand; copying the product's bytes unchanged is always allowed.
    Only keys count: a declared key holding `null` or its default is never a newer field. Opaque objects
    (`id_inputs`, `metadata`, `extra`, `generation.config`, `blocks[].spec`, `ingest_transform[].params`)
    hold data, not format, and their keys are never newer fields.
    Whether a key is a newer field is decided by where it sits: it is one iff the record that holds it doesn't
    declare it. A reader MUST NOT decide it by matching a textual path, since a key's name can spell another
    key's path (a top-level key named `members[0].kind`). When a reader names a newer field by its path, the
    path is written so no two paths read the same: a key of ASCII letters, digits and `_`, not starting with a
    digit, is written bare (after a `.` unless first), any other key as its JSON string literal in brackets, and
    an array index as `[n]`: `producer.agents`, `blocks[0].stats`, `["producer.agents"]`, `tables["a.b"].order`.
  - The same rules cover a **collection** (`collection.json`, ADR-0033): its `manifest_hash` is computed over
    the collection object as parsed with only `manifest_hash` removed, both hashes are required, it MUST be
    I-JSON, and a key the reader doesn't model (top level, `level`, `members[]`) verifies, is warned about
    and blocks a re-seal.
  - An **embedded schema** (`schema`, §5) is different: it is a rule set, not data. A reader that finds a
    key it doesn't model anywhere in it (top level, `fields[]`, `blocks[]`, `tables.<role>` and its
    `columns[]`/`references[]`, `arrays.<role>`; a field's `default` is opaque) MUST NOT report the product
    valid against it. It still verifies the seal and reads the product; it refuses only to validate.
  - **Objects only, one spelling per value.** Every record this spec defines as an object (the manifest and
    each record of §5, a collection with its `level` and `members[]`, the embedded schema with each of its
    records) is a JSON object, and every enumerated name (`kind`, `role`, `order`, `sensitivity`, …) is a
    string. A reader MUST refuse the array form of a record (`["t","u8"]` for `{"name":"t","dtype":"u8"}`),
    which a positional deserialiser would read as the same record, and a name spelled as an object
    (`{"array": null}` for `"array"`): each is a second spelling of one value, and the array form hides its
    keys from the forward-reading rule above, so an unknown rule inside it would be dropped unseen. The rule
    holds for a key a writer omits when it holds its default (✱ in §5; a member `kind` of `product`, a table
    `order` of `sequence_key`) too: written explicitly, its value is that default in its own spelling (`null`,
    `"product"`, `[]`, `false`), never an object or array standing for it (`{"product": null}`). Such a
    manifest or collection is refused before any seal is checked; such an embedded schema is refused like
    one that doesn't parse.

## 5. Manifest schema
A JSON object; so is every record below (BlockRef, Source, Producer, Generation, IngestTransform): objects
only, the array form is refused, and `kind` is a string, never an object (§4). A reader MUST refuse a manifest
whose declared field, at any depth, holds a value of another JSON type than the table gives (`metadata: []`,
`study: 5`, a `kind` outside its names, a non-string `id_inputs` value; `schema` and a block's `spec` are opaque
here and hold any JSON, see §4 for the schema) or lacks a field the table doesn't mark
✱ or give a default (`blocks`, `sources`, `metadata`, `extra`, `ingest_transform` default to empty).
Fields (✱ = omitted from the
JSON when the writer's value is absent/null). Note `[]` and `{}` are **present** values, not omissions: e.g.
`sources: []` stays in the manifest and hashes differently from its absence — only the null/absent sentinel is
dropped.
| key | type | notes |
|---|---|---|
| `tessera_version` | string | spec version, `MAJOR.MINOR.PATCH`. Reader MUST refuse `MAJOR` > supported. |
| `id` | string | §4 |
| `id_inputs` | object<string,string> | the inputs `id` is hashed over |
| `product` | string | schema name (`recon`, `listmode`, …) |
| `name` | string | |
| `description` | string | |
| `timestamp` | string | RFC 3339, normalized to UTC (`…Z`) |
| `producer` ✱ | Producer \| string | who/what wrote it (ADR-0058 §1); older products: a `"tool/version"` string |
| `generation` ✱ | Generation | how the product was made (ADR-0058 §2); required by schemas with `requires_generation` |
| `schema` ✱ | object | the embedded product schema the product validates against (§4 for unknown keys) |
| `blocks` | array<BlockRef> | order is significant (fixes the Merkle order) |
| `sources` | array<Source> | provenance DAG edges |
| `study` ✱ | string | exam-grouping id |
| `metadata` ✱ | object | product-schema field values (keyed by field id) |
| `extra` ✱ | object | non-standard extension namespace |
| `ingest_transform` ✱ | array<IngestTransform> | recorded ingest normalisations, in order (ADR-0056 §6.2) |
| `content_hash` ✱ | string | §4; present once sealed |
| `manifest_hash` ✱ | string | §4; present once sealed |

**BlockRef** = `{ name: string, kind: "array"|"table"|"chunk_index"|"blob", digest✱: string, spec: object }`.
`name` MUST follow the block-name grammar of [ADR-0063](../../docs/adr/0063-block-name-grammar.md): one or
more `/`-separated segments of `[A-Za-z0-9._-]+`, no segment `.` or `..`, at most 255 bytes. A reader MUST
refuse a name outside it. Block names are **unique** within a product: no two BlockRefs of one `blocks`
array have the same `name`, compared as exact strings (byte for byte; `Data` and `data` are two names).
A writer MUST NOT seal such a list, and a reader MUST refuse a manifest that lists a name twice, before it
looks up any digest: a reader resolves a name to one listed digest, so a second entry's digest would cover
no bytes. Two distinct names may share one digest: each name maps to its own `blocks/<name>` member, and each
member is checked. A writer MUST also keep the **leaf rule**: no name in one product is a parent
prefix of another (`volume` and `volume/1` can't both exist), so the zip members, an exploded directory
and an object-store prefix mirror the names one-to-one. A reader accepts a product sealed before the leaf
rule in-zip, but MUST NOT explode it to a directory.
`digest` MUST be present in a sealed product: a reader refuses a manifest with a block that lacks one.
`spec` is shape-specific and opaque to the spine (an array
spec carries `shape`/`dtype`/`chunks`/`axes`/`codec`/…; a table spec carries `columns`/`rows`/…; a
`chunk_index` sidecar spec carries `class`/`recipe`/`indexes`/`entries`/`root` — see §5d; a `blob`
spec carries `filename`/`media_type`/`size` — see §5e).
**Source** = `{ role: string, reference: string, content_hash✱: string }`.
**Producer** = `{ tool: string, version: string, git_commit✱: string, git_repo✱: string, dirty✱: bool }`.
**Generation** = `{ config✱: object, config_ref✱: string }` (`config` is opaque; `config_ref` is a `blake3:`
digest of a config block carried in the product). **IngestTransform** = `{ name: string, params✱: object }`
(`params` is opaque).

## 5a. Array block payload (Zarr v3 + pluggable codec)
A block with `kind == "array"` stores its samples as a **single serialized Zarr v3 store** (ADR-0023).
The array spec carries `shape` (per-axis element counts, C order), `dtype`, `chunks` (cubic chunk
shape, default 64³), `axes`, and `codec` (one of `"pcodec"`, `"zstd"`, `"auto"`).

- **Codec.** The Zarr array uses one of:
  - `"pcodec"` (**default**, the settled imaging-volume codec — lossless, smallest on real CT/PET) —
    the pcodec array→bytes codec.
  - `"zstd"` — the Zarr v3 `bytes` array→bytes codec with the `zstd` bytes-to-bytes codec applied,
    at a **fixed level (3 = the zstd library default)**. The level is hard-coded so the bytes are a
    pure function of the data — never a writer knob or build profile.
  - `"auto"` — a **write-time selector**: the writer encodes the block with BOTH `"pcodec"` and
    `"zstd"`, keeps the smaller payload, and records the *concrete* codec in the manifest's
    `BlockRef.spec`. Ties resolve to `"pcodec"` (the project default). The selection is therefore a
    pure function of the data, so writer-determinism still holds. **Readers never see `"auto"`** in
    a sealed manifest — `BlockRef.spec.codec` is always a concrete codec id.

  Supported dtypes are the full numpy **fixed-width numeric ladder plus bool**, for every codec:
  `int8/16/32/64`, `uint8/16/32/64`, `float16/32/64`, `bool`. The sub-16-bit dtypes are stored
  **widened** to a pcodec-native width — `int8`/`uint8`/`bool` to 16-bit ints, `float16` exactly to
  `float32` — because pcodec operates on ≥16-bit numbers while label/segmentation volumes are
  canonically `uint8` and masks `bool`. The writer widens transparently, `spec.dtype` records the
  logical dtype (the source of truth; the zarr metadata inside the payload declares the widened
  storage dtype), and the reader narrows back, rejecting out-of-range values as corruption (`bool`
  is strict 0/1; `float16` is bit-roundtrip-checked). The widening is lossless by construction and
  free in output size (pcodec absorbs the redundancy). Deliberately excluded, each with a reason:
  `complex64/128` (MRI k-space; plane-split vs interleave is an open storage-design fork),
  `datetime64` (= `int64` + epoch/unit metadata), `bfloat16` (not numpy-native; on demand), and
  strings (table blocks). Float
  bit patterns (incl `NaN`, `±inf`, `−0.0`, denormals) MUST be preserved exactly for all three codec
  ids — bit-exact lossless is a property of the format, not of the codec choice.
- **Codec choice does NOT affect slice/ROI access.** Slice / sub-cube locality is a property of the
  64³ **chunk grid**, not of the codec: both `"pcodec"` and `"zstd"` operate per chunk, so a z-slice
  or 3-D sub-region decodes only the intersecting chunks regardless of which codec is in use. Picking
  `"zstd"` or `"auto"` for a dense / high-entropy auxiliary array does NOT penalise main-volume
  slice-viewing performance.
- **Guidance.** `"pcodec"` is the default and the right choice for real imaging volumes (CT/PET);
  `"zstd"` and `"auto"` are opt-in for dense / high-entropy tabular or auxiliary arrays where
  pcodec's numerical-pattern heuristics gain little. On real CT/PET, `"auto"` naturally picks
  `"pcodec"` (pcodec compresses real medical volumes smaller — CT ~3.8× vs zstd ~3.3×), so `"auto"`
  will not silently degrade an imaging volume.
- **Store serialization.** The Zarr store (its `zarr.json` metadata entry + chunk entries) is
  serialized into one byte string: collect all store keys, **sort by UTF-8 byte order**, then for each
  emit `u32_le key_len · key_utf8 · u64_le value_len · value`. The block payload is exactly this byte
  string; its `digest` (§3) is computed over it. Decoding reverses the framing to rebuild the store.
  Decode is **codec-agnostic**: the store's `zarr.json` declares its codec chain, so a reader needs
  no codec dispatch — only the relevant codec features compiled in (the reference reader bundles
  both pcodec and zstd).
- **Determinism.** Sorted keys + fixed framing + deterministic Zarr-v3 metadata + deterministic
  codec (pcodec is deterministic; zstd at a fixed level is deterministic; `auto`'s pick-smaller is a
  pure function of the resulting byte lengths) ⇒ the same array always serializes to byte-identical
  payload bytes (the writer-determinism gate).

A reader MAY decode only a sub-region by reconstructing the store and reading the intersecting chunks
— independent of the codec.

## 5b. Table block payload (Vortex)
A block with `kind == "table"` stores its columns as a **single chunked Vortex file** (ADR-0024,
ADR-0026) — the exact bytes a [Vortex](https://github.com/spiraldb/vortex) reader opens. The table
spec carries an ordered `columns` list (`name`, `dtype`, optional `codec`), a `rows` count, and an
optional `row_index`.

- **Row groups.** The table is **always** written as a chunked Vortex array with a fixed
  **`ROWS_PER_GROUP = 65536`** rows per chunk (the last chunk is the remainder; an empty table is one
  empty chunk). The fixed grid makes the layout a pure function of the data — so the **batch** writer
  and a **streaming/>RAM** writer that flushes one row-group at a time produce *identical* bytes (one
  encoder). A table with ≤ 65536 rows is a single row-group, whose bytes are **identical to the prior
  single-array layout** — so this is backward-compatible (existing products unchanged); only larger
  tables gain extra chunks.

- **Block partition (>RAM tables).** A logical table with more than a fixed
  **`BLOCK_ROWS = 2^22 = 4_194_304`** rows is partitioned across **multiple blocks** in the manifest:
  the first `BLOCK_ROWS` rows are one block, the next `BLOCK_ROWS` rows the next block, and so on
  (the trailing block may be partial). Each block carries one of the chunked Vortex payloads above,
  with the per-block row count in its `TableSpec.rows`. Naming follows the **small-stays-single**
  rule:
  - **One block total** (rows ≤ `BLOCK_ROWS`) — the manifest block is named with the bare prefix
    (e.g. `events`). This is byte-identical to the pre-partition layout, so existing single-block
    products are unchanged.
  - **Two or more blocks total** — the blocks are the **leaves** `<prefix>/<leaf>` of the node `<prefix>`
    (ADR-0063 names), where the leaf of the block with index `i` (from `0`) is the decimal `i` left-padded with
    `0` to **at least** four digits and no more than needed: `<prefix>/0000`, `<prefix>/0001`, …,
    `<prefix>/9999`, `<prefix>/10000`, …. The width depends on the index only, never on the block count, so
    there is no ceiling and a writer can name a block before it knows how many follow. Every leaf's
    `TableSpec` carries a **partition marker** `"partition": {"index": i, "last": b}` (both fields always
    present; `last` is `true` on exactly the leaf with the highest index). A single block carries no marker.
    The leaves MUST appear in the manifest in strictly increasing index order (other blocks, such as a leaf's
    `.cidx`, may sit between them). The names and markers are bound by `manifest_hash` and not by `content_hash`,
    which covers the payload digests only: the same rows under the flat and the leaf names have the same
    `content_hash` and different manifests. *(Amended by
    [ADR-0064](../../docs/adr/0064-logical-tables-over-numbered-leaves.md), 2026-10-30: the leaves replace the
    flat `<prefix>_0000` … `<prefix>_NNNN` names, which writers no longer produce and readers still accept,
    below. ADR-0064 is Accepted (2026-11-02); every multi-block writer writes the leaves since its PR D, #712.)*
  Both whole-file and streaming writers MUST use the same partition function (`ceil(rows / BLOCK_ROWS)`,
  with `max(1)` so an empty table is still one block) and the same naming function — so streamed and
  batch ingest of the same rows produce byte-identical per-block bytes and therefore the same
  `content_hash`, independent of any runtime knob (`workers`, `ram_budget`, slab size).
  `BLOCK_ROWS` is chosen at exactly 64 × `ROWS_PER_GROUP` so every full block is exactly 64
  row-groups (whole-multiple invariant). **Changing `BLOCK_ROWS` is a format-breaking change**
  (different partition → different per-block bytes → different `content_hash` for every product with
  more than one block).
  The partition binds the engine's table writers (ingest, streaming writers, multi-block sinks). A low-level
  constructor that writes one caller-named block from data already in memory (`table::table_block`, the
  Python `Builder.add_table`) writes that one block at any row count, and readers MUST accept a single table
  block of any row count (ADR-0064 §5.6).

- **Dtypes.** Columns use the fd5 numpy-style codes: `i1/i2/i4/i8` (signed), `u1/u2/u4/u8` (unsigned),
  `f4/f8` (float). Each column is a Vortex primitive array; the columns form a `struct` in declared
  order, every column of length `rows`. Float bit patterns (`NaN`, `±inf`, `−0.0`, denormals) MUST be
  preserved exactly.
- **Payload.** The block payload is exactly the serialized Vortex file (data arrays + per-column
  statistics + a flatbuffer footer); its `digest` (§3) is computed over those bytes.
- **Determinism.** The Vortex writer is byte-deterministic for fixed input + version, **with the ALP
  float schemes excluded** from the compressor (`vortex.float.alp` / `vortex.float.alprd`): ALP's
  exponent search is float-codegen-sensitive and otherwise makes the bytes depend on the writer's
  build profile. Excluding it makes the payload a pure function of the logical data (floats store
  flat/Pco). Cross-*version* stability is still not guaranteed pre-1.0 — pin the Vortex version
  (recorded in the corpus provenance).
- **Edition.** `TableSpec.edition` (optional string, e.g. `"core2026.08.3"`) names the Vortex
  **edition** the payload was written under: the frozen set of encoding wire IDs the writer was allowed to
  emit *(ADR-0024, 2026-11-15 amendment)*. A writer MUST set it to the edition it encoded under, on every
  table block and partition leaf, and MUST NOT record an edition it did not write under. It is bound by
  `manifest_hash` (it is in the manifest) and not by `content_hash` (it is in no payload). **Absent** means
  the block predates the field (written by the vortex 0.75 writer, which had no edition gate); such a block
  is valid, reads and verifies, and its manifest bytes are unchanged, because the key is omitted, never
  written as `null`. A reader MUST NOT need it to decode: Vortex decodes by the wire IDs in the file. Same
  input under a **different** edition may give different bytes; the **same** edition is necessary for the same
  bytes but not sufficient, since the edition bounds which encodings the writer may use, not which it picks
  (that is the writer's codec set, ADR-0024 §Codec set, and the pinned Vortex compressor, both changed only as
  a format event).
- **Codec profile.** `TableSpec.codec_profile` (optional string, `tessera-table-v<N>`, today
  `"tessera-table-v1"`) names the writer's **codec set**: which of the edition's schemes the compressor may choose
  from, together with the pinned Vortex compressor that chooses among them *(ADR-0024, 2026-11-16 amendment)*. A
  writer MUST set it to the profile it encoded with, on every table block and partition leaf, beside `edition`, and
  MUST NOT record a profile it did not encode with. Like `edition` it is bound by `manifest_hash` and not by
  `content_hash`. **Same input + same edition + same profile → same bytes**: two table blocks of the same rows that
  record the same edition and the same profile MUST have the same payload bytes. The profile says nothing about
  decodability, which the edition governs. A reader MUST NOT refuse a table block because its profile is
  **unknown** or **absent**; it decodes such a block as any other. Absent means the block predates the field
  (every product before #869, `0.1.0-alpha.5` included); the key is then omitted, never written as `null`, and the
  block's manifest bytes are unchanged. A profile id is bumped on any change to the codec set or on a Vortex change
  to the compressor's choices, each a format event.
- **Forward-read limit.** Reading is backward-compatible only: a newer reader decodes older table blocks, but
  an older reader can't be assumed to decode newer ones. Decoding a table block written under an edition needs
  a Vortex at or above that edition's minimum library version (for `core2026.08.3`, Vortex 0.85). In tessera
  that means a reader at or after the vortex 0.93 change (ADR-0024's 2026-11-09 amendment, #635), the first
  release after `0.1.0-alpha.5`. The edition field is how a reader knows which edition a block needs. A tessera
  reader from before that change (vortex 0.75; `0.1.0-alpha.1`–`alpha.5`) still opens and **verifies** such a
  product, because §7 needs no payload decoding. But it fails the table **decode** with a codec registry error
  (`Encoding not found in registry`), and the error doesn't name the edition. Array, blob and table-free
  products aren't affected. A reader that can't decode an edition MUST fail with an error, never return
  partial or substituted data; naming the edition in that error is #871.

A reader MAY project a subset of columns or random-`take` rows; the format supports O(1) random access.

- **Logical reads (multi-block).** A table name `T` resolves, from the manifest alone, to one **logical
  table** *(amended by [ADR-0064](../../docs/adr/0064-logical-tables-over-numbered-leaves.md) §3,
  2026-10-30)*:
  - **Partitioned.** If some block exactly one segment below `T` (a name `T/<segment>`, no further `/`)
    carries a partition marker, `T` is a partitioned table, and it MUST be valid: every such marked block is a
    `table` block whose segment is the canonical padded form of its own `index`; every other block under `T/`,
    at any depth, belongs to a child class ADR-0064 §2.1 registers (today only the `chunk_index` sidecar
    `T/<leaf>.cidx` of an existing leaf); the indices are unique and form `0..=k` with `k ≥ 1`; exactly the leaf
    `k` has `last: true`; the leaves appear in the manifest in increasing index order; and every leaf has the same
    column names and dtypes. Its blocks, in **index order** (which is their manifest order), are the logical
    table.
    A reader MUST refuse an invalid partitioned table with an error, never skip or merge around a block.
    Per-leaf row counts are taken from each `TableSpec.rows` and are not otherwise checked.
  - **Legacy flat** (products written before ADR-0064). Otherwise, the blocks named `T` or `T_` followed by
    exactly four ASCII digits, in **manifest order**, are the logical table; blocks under `T/` without a marker
    take no part. Readers MUST keep accepting this form for format v0. Writers MUST NOT produce it once ADR-0064's
    writer switch (its PR D) has landed; until then the writers still produce it.
  - **Ambiguous.** If `T` has a marked block one segment below it and the legacy rule also matches some block, a
    reader MUST refuse `T`.
  - A marker on a block with no parent node is invalid; a node with no marked child one segment below it (an
    array pyramid, `volume/0` …) is not a table.

  The logical column is the ordered concatenation of the per-block columns; a
  global row maps to a `(block, local_row)` pair via the cumulative per-block row counts (drawn
  from each block's `TableSpec.rows` in the manifest, so the mapping needs no block read). A
  reader MAY answer ranged queries with **block-level pruning** — the per-block `[min, max]`
  rolled up from the block's chunk-index sidecar (§5d, no data-block bytes read), or from a
  single column-projection scan when no sidecar is present — and skip the blocks whose
  `[min, max]` cannot overlap the requested range.

## 5c. Referenced coordinates & quantities (ADR-0032)
How a stored index or sample value maps to a *physical* coordinate or quantity is described by **one**
optional descriptor — `referencing = (transform, unit, frame)` — never by bespoke per-domain fields.
It is **optional → feature-by-presence**: no descriptor means index/raw space (a sinogram axis, a
unitless count), not "unknown". The descriptor is **meaning-bearing**, so it lives in the manifest under
`manifest_hash`; the stored bytes stay raw and lossless (the transform is applied **downstream at read**,
store-don't-compute).

- **`transform`** — a closed, self-describing (`kind`-tagged) taxonomy, the case chosen by an axis's
  *coupling* and *regularity*, not its position:
  - `identity` — the index *is* the coordinate.
  - `affine_1d` — `physical = scale·stored + offset` (intensity rescale HU/Bq·mL⁻¹, a regular time axis,
    regular bins, integer-tick→seconds).
  - `affine_nd` — a coupled `dims × (dims+1)` row-major matrix `[R | t]` (the spatial voxel→world affine
    of §5a's `world_frame`; implicit last row `[0…0 1]`).
  - `lookup` — explicit per-index values (irregular bins/b-values/PET frame mid-times).

  `affine_1d` and `affine_nd` are **one operator at different rank** — a scalar rescale is the 1-D case
  of the spatial matrix; the format implements affine-apply once.
- **`unit`** — a **UCUM** code (`mm`, `s`, `Bq/mL`, `keV`, `1`); the dimension is derivable. A
  `vocabulary` escape (fd5 `_vocabulary`/`_code`) carries a non-UCUM domain code. The validated UCUM
  subset is pinned (`CANONICAL_UNITS`); the format stores the code and runs **no** units engine.
- **`frame`** — the named reference for the origin/zero, from the pinned vocabulary (`CANONICAL_FRAMES`)
  optionally refined with a `:<detail>` suffix: spatial `patient | scanner | aligned | atlas:<id>`
  (+ LPS convention, §5a); temporal `epoch` / `epoch:<instant>` (`unix`/`acquisition_start`/`injection`);
  intensity `physical` / `baseline:<name>` (`hounsfield`, `decay_corrected@<instant>`).
- **Instances** — spatial geometry = `affine_nd`/`mm`/`space`; regular time = `affine_1d`/`s`/`epoch`;
  irregular time or event times = `lookup`/`s`/`epoch`; intensity = `affine_1d`/UCUM/baseline; parametric
  = `affine_1d`|`lookup`/UCUM/named. Quantities needing *other* data (SUV, ADC) are **derived products**,
  not descriptors.
- **Time split (#220).** Wall-clock absolute time (acquisition datetime, provenance) is a **UTC RFC 3339**
  string in metadata; elapsed/relative time (durations, frame offsets, event times) is a **quantity on a
  monotonic scale** (`affine_1d`/`lookup`, unit `s`, an `epoch` frame). High-rate event timestamps are
  **integer ticks + scale** (`tick→s`), lossless and compact — never floats. An absolute instant = an
  elapsed quantity **+** its `epoch`.
- **Placement.** A per-**value** descriptor and per-**axis** descriptors attach to the array spec (the
  value/spatial ones are *derived* from §5a's `rescale_*`/`world_frame` — single source of truth, not
  duplicated; per-axis descriptors are stored explicitly). All are serialized inside the block spec, so
  they are covered by `manifest_hash` and deterministic (`f64`/`int+scale`/canonical RFC 3339).

## 5d. Chunk-index sidecar block (ADR-0028 §3/§4)
A block with `kind == "chunk_index"` is an **additive, derived companion** to a data block: the
`{hash, stats}` index over that block's ordered sub-blocks (array chunks in C-order over the chunk grid,
or table row-groups). It is **optional** — present only when a writer chooses to emit it (e.g. the
`*_indexed` streaming path); absent for a plain product, which is byte-identical without it.

- **Naming.** The sidecar for a data block `<name>` is the sibling block `<name>.cidx`.
- **Payload.** Canonical JSON of the index: `{ "entries": [ { "digest": string, "stats": { "count": u64,
  "min": i64|null, "max": i64|null, "sum": i128, "sum_sq": i128 } }, … ] }` — one entry per sub-block, in
  storage order. The block `digest` is `digest(payload)` like any block, so it rolls into `content_hash`
  (the sidecar is integrity-covered, not free-floating).
- **Spec (self-description).** `{ "class": "derived", "recipe": "chunk_index@1", "indexes": "<name>",
  "entries": <count>, "root": "blake3:…" }`. `class:"derived"` + `recipe` mark it **regenerable** from the
  indexed data block (a consumer MAY drop and rebuild it). `root` is the sub-block MMR root over the
  per-entry digests (the §3 construction), i.e. the data block's per-chunk confirmable Merkle root.
- **Use.** A consumer reads the sidecar for **per-chunk inclusion proofs** and **predicate pruning**
  (skip a sub-block whose `[min,max]` cannot match a query) without re-reading the data; the rolled-up
  `stats` are the leaves of the multiscale aggregate pyramid (ADR-0028 §3). A consumer that doesn't need
  these ignores the block — `kind`-dispatch makes it inert to a spine-only reader.

## 5e. Blob block payload (opaque preservation tier, ADR-0038)
A block with `kind == "blob"` stores a file's bytes **verbatim** — no codec, no decode. The payload at
`blocks/<name>` **is** the original bytes; the `digest` is `blake3(bytes)` like every block, so the same
seal + signature + reader verification (§7) apply unchanged and prove bit-faithfulness. The spec is
`{ filename: string, media_type✱: string, size: u64 }`. This is the **preservation tier** for the long
tail of vendor output the engine does not parse (Siemens `.l64` listmode, GE `.7z`/`.cal`, PDFs, logs):
a blob is Findable, Accessible, Reusable-as-bytes and integrity-verified, but **not Interoperable** until
a decoder exists — so it complements, never replaces, the normalised array/table products (a raw blob and
a derived product may coexist in one product, joined by a `derived_from` edge). Recovered byte-identical
via `tessera extract <tsra> <block> <out>`.

## 6. Container `.tsra`
A ZIP archive (zip64). Entries, in order:
1. **`mimetype`** — STORED (uncompressed), first entry, content exactly `application/vnd.tessera`
   (the magic; lets sniffers identify without unzipping).
2. **`manifest.json`** — the manifest (§5), any valid JSON.
3. **`blocks/<name>`** — one entry per block, the block's payload bytes.

The archive MUST begin at byte 0 of the file: no prefix, so the first local file header is `mimetype`'s.
Sniffing depends on this, and so does a cheap open. A reader may take the archive offset as 0 and check
the one position the end-of-central-directory record names, never scanning the file for it. A reader
MUST refuse an archive whose first central-directory entry is not `mimetype`, or whose `mimetype` local
header is not at byte 0. The second rule also covers a prefix whose offsets were rewritten to include it
(a self-extractor or a shell/zip polyglot), which is otherwise a well-formed zip.

**One reading.** Every zip entry is described twice, by its local file header and by its central-directory
record, and zip readers differ in which one they trust and in how they find the archive at all: a
central-directory reader (this reference implementation, Python's `zipfile`, `unzip`) never looks at a local
header's name or method, a streaming reader never sees the central directory, and a reader that finds the
end-of-central-directory record by scanning back from the end of the file and adjusts for a prefix opens
whatever archive ends last. Readers also differ in which fields they act on: a streaming reader looks for a
member's end where a data-descriptor flag says it is, some readers rename a member after a Unicode-path extra
field, decrypt after an encryption flag, write a symlink after a Unix mode, or take `\` for a separator after
a DOS host byte. A `.tsra` MUST therefore read the same every way (GHSA-m665-v7pm-7grj), which holds only if
every byte of the file belongs to exactly one structure and no field is left for a reader to interpret. The
**canonical layout** is:

1. **Members.** The local entries (local header, then the member's bytes), back to back from byte 0. Sorted
   by position, the first starts at byte 0, each starts where the previous one's bytes end, and the last ends
   where the central directory starts. No gap, no overlap.
2. **Canonical encoding.** Every local file header and every central-directory record is, byte for byte, the
   one the reference writer emits for that member: every field below is either the value shown or derived
   from the member's name, CRC-32, size and local-header offset.

   | Field | Local header | Central-directory record |
   | --- | --- | --- |
   | version made by | (none) | `0x032D`: host 3 (Unix), version 4.5 |
   | version needed to extract | `45` (4.5, zip64) | `45` |
   | general-purpose flags | `0`: no encryption, no data descriptor, no UTF-8 flag (names are ASCII) | `0` |
   | compression method | `0` (STORED) | `0` |
   | modification time, date | `0x0000`, `0x0021` (1980-01-01 00:00:00) | the same |
   | CRC-32 | the member's | the member's |
   | 32-bit compressed, uncompressed size | `0xFFFFFFFF`, `0xFFFFFFFF` | `0xFFFFFFFF`, `0xFFFFFFFF` |
   | extra field | exactly one zip64 field (`0x0001`), 16 data bytes | the same; 24 past 32 bits (below) |
   | comment, disk number start, internal attributes | (none) | empty, `0`, `0` |
   | external attributes | (none) | `0x81A40000`: a regular file, mode `0644` |
   | 32-bit local header offset | (none) | the offset, or `0xFFFFFFFF` when the offset is `0xFFFFFFFF` or more |

   The zip64 field holds the uncompressed size, then the compressed size (equal, since STORED). In the record it
   also holds the local header's offset (24 bytes in all) when that offset is `0xFFFFFFFF` or more. So there is
   no data descriptor, no encrypted, directory or symlink entry, no extra field but zip64's and no entry
   comment, and a local header agrees with its record by construction.
3. **Central directory.** Exactly one record per entry, back to back, no name twice, listed in the order the
   entries sit in the file. A reader walking the directory and one walking the local headers then meet the
   members in one order, so an extracting reader leaves the same tree either way.
4. **End records.** Directly after the central directory, exactly the records the reference writer emits.
   The zip64 end-of-central-directory record (`44`-byte record, versions `45`, no extensible data sector) and
   its locator come first if and only if the archive has more than 65,535 entries or a directory size or
   offset above `0xFFFFFFFF`. Then the end-of-central-directory record: disk numbers `0`, one disk, the entry
   count (`0xFFFF` past 65,535), the directory size (`0xFFFFFFFF` when the zip64 records are present) and the
   directory offset (`0xFFFFFFFF` when it is `0xFFFFFFFF` or more), and the comment length `0`. Every zip64
   field states the true value, and the locator points at the zip64 record.
5. **Nothing else.** The archive comment is empty, and the end-of-central-directory record ends at the end
   of the file. Bytes after it, or a comment, can hold a second archive that a prefix-adjusting reader opens
   instead of this one.

**Members a `.tsra` may hold.** Exactly `mimetype`, `manifest.json`, one `blocks/<name>` for each block the
manifest lists, and zero or more `aux/<name>` members (ADR-0042). Any other entry, a `blocks/` member the
manifest does not list included, MUST be refused: no digest covers its bytes, and every reader would take it
for part of the product. The `<name>` of an `aux/` member obeys the block-name grammar (ADR-0063): one or
more `/`-separated segments of `[A-Za-z0-9._-]`, none empty, `.` or `..`, at most 255 bytes. So no `..`, no
leading or doubled `/`, no `\`, no NUL or control character, no non-ASCII byte, and no directory entry (a
trailing `/`). Across one archive, the `aux/` names compared with ASCII letter case folded are distinct, and
none is a parent prefix of another (the leaf rule: not `aux/a` next to `aux/a/b`). So an extracting reader
writes every member under its target directory, as a plain file, under the one name every reader decodes, and
leaves one tree whatever order it writes them in. (Windows device names such as `CON` and a trailing `.` stay
grammar-valid; #690 tracks them, with case collisions among `blocks/` names, before a Windows build ships.)
Every `aux/` member's bytes MUST match its CRC-32: a reader that checks the CRC refuses or truncates a
mismatched member while another extracts it. `aux/` is the one place for bytes outside the seal (the embedded
signature, the ingest provenance stamp). Because the layout above holds, every reader sees the same `aux/`
members with the same bytes as it sees the sealed ones, but verification attests only the sealed members. An
`aux/` member is carried, not authenticated, and a consumer MUST NOT treat its content as covered by
`content_hash` (an embedded signature authenticates itself against `manifest_hash`, ADR-0037). It is written
in the same canonical encoding as every other member, so it is added by the reference writer (`tessera sign`,
`tessera pack` of an exploded directory, the library's `add_aux_members`), not by a generic zip tool.

A reader MUST refuse to read a member whose local header disagrees with its record, and MUST refuse a member
read from an archive that names one member twice. Full verification (§7) MUST check the whole canonical
layout, encoding included, and the member set. Only the whole-file check catches a complete local entry
hidden in bytes no record points at, a field a reader would interpret, or a second archive after the end
record. Full verification also reads every `aux/` member to check its CRC-32, since nothing else reads an
unsealed member. A cheap open need not check any of it, since it reads every local header and the whole
central directory.

All entries MUST be **STORED** (payloads are pre-compressed by their codec; STORED keeps every block
byte range directly addressable via the central directory). For **writer-determinism**, entry
modification times MUST be pinned to the zip epoch (1980-01-01 00:00:00) — the same product MUST pack
to byte-identical archive bytes. Both rules are part of the canonical encoding above, so full verification
enforces them. The zip **central directory** is the internal index: a cloud reader
ranged-reads it, then ranged-reads only `manifest.json` and the block entries it needs.
An opt-in **exploded** form is the same tree unzipped under an object-store prefix.

## 7. Verification (reader procedure)
On open a reader MUST: (a) read `mimetype`, reject if ≠ `application/vnd.tessera`; (b) parse
`manifest.json`; (c) reject if `tessera_version` major > supported, or if `blocks` lists a name twice
(§5); (d) recompute and check `id` from
`id_inputs`, `content_hash` from block digests, and `manifest_hash` over the parsed manifest object
(§4) — any mismatch is an integrity failure; a duplicate key is refused before any hash is computed. On reading a block, the reader MUST verify the bytes against the
BlockRef `digest`. Full verification of a file also checks the container's canonical layout and member
set (§6, "One reading"), so a `.tsra` that verifies reads the same under every zip reader, byte for byte.

## 8. Versioning
Products are immutable. A new version is a new product (new `id`) with a `Source`
`{ role: "supersedes" | "derived_from", reference: <parent id>, content_hash: <parent manifest_hash> }`,
forming a tamper-evident Merkle DAG.

## 9. Conformance
`tessera/corpus/corpus.json` lists golden `{name, product, id, content_hash, manifest_hash}` for the
reference fixtures, and `tessera/corpus/files/<name>.tsra` holds the corresponding **concrete `.tsra`
test vectors** an independent implementation reads directly. A conformant implementation MUST, for
each test vector: open it, recompute `id` / `content_hash` / `manifest_hash` per §2–§7, and match the
golden triple. Reproducing the triple needs only the container (§6) + canonical hashing (§2–§4) —
**not** block-payload decoding — so a reader can be validated without the array/table codecs. Both are
regenerated (and the spec version bumped) only on an intended format change:
`cargo run -p tessera-io --example gen_corpus` / `--example gen_corpus_files`.

`tessera/corpus/seal-cases.json` adds manifest-only vectors for §4's seal rule: each corpus manifest with a
newer-format key added and resealed (verifies), the same key spliced in under the old seal (fails), the
encodings a typed model loses (`"study": null`, an omitted `sources`, a nested newer key; all verify) and a
splice whose seal was deleted or nulled (fails), text that is not I-JSON (refused), and a block, the producer
or the whole manifest spelled as a JSON array, or a block `kind` as an object (refused, §4), a declared field of
another JSON type or shape, a block without a digest, and nesting past 127 levels (refused, §4/§5). A conformant
reader MUST reach every case's `verdict` from its `manifest` text.
