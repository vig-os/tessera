# Tessera format specification (v0, pre-1.0)

Language-neutral. An independent implementation built from this document alone must reproduce the
golden hashes in `tessera/corpus/corpus.json` (the v1.0 conformance gate). Normative keywords:
**MUST** / **SHOULD** / **MAY**. Authoritative decisions: `docs/adr/0020`, `docs/adr/0022`.

## 1. Overview
A Tessera **product** is one immutable, content-hashed, self-describing FAIR data artifact: a JSON
**manifest** + zero or more shape-dispatched storage **blocks** (N-D arrays, columnar tables). The
canonical serialized form is a single sealed **`.tsra`** container.

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

## 5. Manifest schema
A JSON object. Fields (✱ = omitted from the JSON when the writer's value is absent/null). Note `[]`
and `{}` are **present** values, not omissions: e.g. `sources: []` stays in the manifest and hashes
differently from its absence — only the null/absent sentinel is dropped.
| key | type | notes |
|---|---|---|
| `tessera_version` | string | spec version, `MAJOR.MINOR.PATCH`. Reader MUST refuse `MAJOR` > supported. |
| `id` | string | §4 |
| `id_inputs` | object<string,string> | the inputs `id` is hashed over |
| `product` | string | schema name (`recon`, `listmode`, …) |
| `name` | string | |
| `description` | string | |
| `timestamp` | string | RFC 3339, normalized to UTC (`…Z`) |
| `schema` ✱ | object | embedded JSON Schema (opaque) |
| `blocks` | array<BlockRef> | order is significant (fixes the Merkle order) |
| `sources` | array<Source> | provenance DAG edges |
| `study` ✱ | string | exam-grouping id |
| `metadata` ✱ | object | product-schema field values (keyed by field id) |
| `extra` ✱ | object | non-standard extension namespace |
| `content_hash` ✱ | string | §4; present once sealed |
| `manifest_hash` ✱ | string | §4; present once sealed |

**BlockRef** = `{ name: string, kind: "array"|"table"|"chunk_index"|"blob", digest✱: string, spec: object }`.
`digest` MUST be present in a sealed product. `spec` is shape-specific and opaque to the spine (an array
spec carries `shape`/`dtype`/`chunks`/`axes`/`codec`/…; a table spec carries `columns`/`rows`/…; a
`chunk_index` sidecar spec carries `class`/`recipe`/`indexes`/`entries`/`root` — see §5d; a `blob`
spec carries `filename`/`media_type`/`size` — see §5e).
**Source** = `{ role: string, reference: string, content_hash✱: string }`.

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

  Supported dtypes are `int16/32/64`, `uint16/32/64`, `float32/64` for every codec (8-bit and
  `float16` are intentionally out of scope so the dtype envelope does not depend on the codec). Float
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
  - **Two or more blocks total** — blocks are named with a zero-padded 4-digit suffix:
    `<prefix>_0000`, `<prefix>_0001`, …, `<prefix>_NNNN` (in manifest order).
  Both whole-file and streaming writers MUST use the same partition function (`ceil(rows / BLOCK_ROWS)`,
  with `max(1)` so an empty table is still one block) and the same naming function — so streamed and
  batch ingest of the same rows produce byte-identical per-block bytes and therefore the same
  `content_hash`, independent of any runtime knob (`workers`, `ram_budget`, slab size).
  `BLOCK_ROWS` is chosen at exactly 64 × `ROWS_PER_GROUP` so every full block is exactly 64
  row-groups (whole-multiple invariant). **Changing `BLOCK_ROWS` is a format-breaking change**
  (different partition → different per-block bytes → different `content_hash` for every product with
  more than one block).

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

A reader MAY project a subset of columns or random-`take` rows; the format supports O(1) random access.

- **Logical reads (multi-block).** The ordered sequence of `prefix` / `prefix_NNNN` blocks is one
  **logical table**: the logical column is the ordered concatenation of the per-block columns; a
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

All entries MUST be **STORED** (payloads are pre-compressed by their codec; STORED keeps every block
byte range directly addressable via the central directory). For **writer-determinism**, entry
modification times MUST be pinned to the zip epoch (1980-01-01 00:00:00) — the same product MUST pack
to byte-identical archive bytes. The zip **central directory** is the internal index: a cloud reader
ranged-reads it, then ranged-reads only `manifest.json` and the block entries it needs.
An opt-in **exploded** form is the same tree unzipped under an object-store prefix.

## 7. Verification (reader procedure)
On open a reader MUST: (a) read `mimetype`, reject if ≠ `application/vnd.tessera`; (b) parse
`manifest.json`; (c) reject if `tessera_version` major > supported; (d) recompute and check `id` from
`id_inputs`, `content_hash` from block digests, and `manifest_hash` over the canonical bytes — any
mismatch is an integrity failure. On reading a block, the reader MUST verify the bytes against the
BlockRef `digest`.

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
