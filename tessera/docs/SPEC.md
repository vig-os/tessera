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

## 3. Hashes
- Algorithm: **BLAKE3**, 256-bit, output as lowercase hex with the prefix `blake3:`.
  `digest(bytes) = "blake3:" + hex(blake3(bytes))`.
- **Merkle root** over an *ordered* list of digest strings `d[0..n]`:
  `merkle_root = "blake3:" + hex(blake3(d[0] ++ d[1] ++ … ++ d[n-1]))` where each `d[i]` is hashed as
  its UTF-8 string bytes, concatenated in order. The empty list hashes the empty input.

## 4. Identity model (three hashes)
- **`id`** (logical identity): `id = digest(JCS(id_inputs))`, where `id_inputs` is a JSON object of
  string→string. Default keys: `product`, `name`, `timestamp`. Stable across rename / byte re-encode.
- **`content_hash`** (data fingerprint): `merkle_root([block.digest for block in blocks])`, blocks in
  manifest order. Each block's `digest` is `digest(<the block's stored payload bytes>)`.
- **`manifest_hash`** (the seal): `digest(JCS(manifest_without_manifest_hash))` — the manifest object
  with the `manifest_hash` key omitted (all other fields, including `content_hash`, present). It
  transitively commits to every block digest and all metadata.

## 5. Manifest schema
A JSON object. Fields (✱ = omitted when empty/None):
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

**BlockRef** = `{ name: string, kind: "array"|"table", digest✱: string, spec: object }`. `digest`
MUST be present in a sealed product. `spec` is shape-specific and opaque to the spine (an array spec
carries `shape`/`dtype`/`chunks`/`axes`/`codec`/…; a table spec carries `columns`/`rows`/…).
**Source** = `{ role: string, reference: string, content_hash✱: string }`.

## 5a. Array block payload (Zarr v3 + pcodec)
A block with `kind == "array"` stores its samples as a **single serialized Zarr v3 store** (ADR-0023).
The array spec carries `shape` (per-axis element counts, C order), `dtype`, `chunks` (cubic chunk
shape, default 64³), `axes`, and `codec` (`"pcodec"`).

- **Codec.** The Zarr array uses the **pcodec** array→bytes codec (lossless). pcodec operates on
  ≥16-bit numbers: supported dtypes are `int16/32/64`, `uint16/32/64`, `float32/64`. Floating-point
  bit patterns (incl `NaN`, `±inf`, `−0.0`, denormals) MUST be preserved exactly.
- **Store serialization.** The Zarr store (its `zarr.json` metadata entry + chunk entries) is
  serialized into one byte string: collect all store keys, **sort by UTF-8 byte order**, then for each
  emit `u32_le key_len · key_utf8 · u64_le value_len · value`. The block payload is exactly this byte
  string; its `digest` (§3) is computed over it. Decoding reverses the framing to rebuild the store.
- **Determinism.** Sorted keys + fixed framing + deterministic pcodec/Zarr-v3 metadata ⇒ the same
  array always serializes to byte-identical payload bytes (the writer-determinism gate).

A reader MAY decode only a sub-region by reconstructing the store and reading the intersecting chunks.

## 5b. Table block payload (Vortex)
A block with `kind == "table"` stores its columns as a **single Vortex file** (ADR-0024) — the exact
bytes a [Vortex](https://github.com/spiraldb/vortex) reader opens. The table spec carries an ordered
`columns` list (`name`, `dtype`, optional `codec`), a `rows` count, and an optional `row_index`.

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
