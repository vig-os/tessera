# Anatomy of a `.tsra`

A product is a **manifest spine** plus **N typed blocks**. The manifest is the small, range-readable
JSON that says *what the product is*; the blocks carry the bytes.

## The three hashes

Every product carries three identifiers, each with a distinct job (ADR-0020):

| Field | What it is | Changes when… |
|---|---|---|
| `id` | logical identity — `blake3(JCS(id_inputs))` | the *logical* thing changes (never on a byte re-encode or rename) |
| `content_hash` | recursive MMR Merkle root over the ordered digests of the **primary** blocks only (ADR-0028 §4); a derived block (`"class": "derived"`: a `.cidx`, a pyramid level above 0) is not part of it | a primary block's bytes change |
| `manifest_hash` | the seal — `blake3(JCS(manifest))` | *any* manifest byte changes (metadata, sources, any block's digest or class) |

`id` is stable across a re-ingest with a better codec (same logical product → same `id`, new
`content_hash`); `manifest_hash` is the tamper-evident seal a signature attests.

## Block kinds

Blocks dispatch by data shape — one spine, the proven engine per shape:

- **Array** — dense N-D, Zarr v3 grid, 64³ cubic chunks, `pcodec` (lossless). Volumes, μ-maps, sinograms.
- **Table** — columnar Vortex, addressable + filter-pushdown, zero-copy to Arrow. Events, spectra, ROIs.
- **Blob** — an un-parsed vendor file stored **verbatim** (`blake3(bytes)`, no codec), for bit-faithful
  preservation of anything not yet decoded (ADR-0038). See [Ingesting vendor data](./ingest.md).
- **ChunkIndex** — an additive companion carrying per-chunk `{hash, stats}` for pruning + per-chunk
  proofs (ADR-0028 §3). It rides *beside* a data block; the block's own digest stays a flat hash of its
  payload (ADR-0028 §4.1), so identity is independent of internal chunk layout.

## One glance at the manifest

`inspect` prints the identity triple and the per-block digests:

{{#include ../../../tessera/crates/tessera-cli/tests/cmd/inspect.trycmd}}

(The `id` and version lines are globbed with `trycmd` wildcards; the `content_hash` / `manifest_hash` /
block digests are pinned exact — they move only on a deliberate conformance-corpus regeneration.)

## The stored `manifest.json` and its `manifest.seal`

A `.tsra` starts `mimetype`, `manifest.seal`, `manifest.json` (ADR-0066). `manifest.json` (and a collection's
`collection.json`) is stored as exactly its RFC 8785 JCS form: one line, no whitespace, keys sorted, JCS
number and string forms, and no `manifest_hash` key. The seal is `blake3` of exactly those bytes, stored in
`manifest.seal` (`collection.seal` beside a collection), so checking it needs no JSON library. `jq` gives the
readable view:

```sh
unzip -p study.tsra manifest.json          # one line: {"blocks":[...],"content_hash":"blake3:...",...}
unzip -p study.tsra manifest.json | jq .   # the same object, indented
unzip -p study.tsra manifest.seal          # blake3:<64 hex>
unzip -p study.tsra manifest.json | b3sum  # the same 64 hex digits
```

Readers refuse a `manifest.json` that is not its own JCS form (a re-indented file, `64.0` for `64`), even
under a seal recomputed over it: one seal has one byte string and one meaning. A product written before
this layout (no `manifest.seal`) is refused with an error naming the layout it found and the one required;
there is no migration, so re-ingest it from its source.
