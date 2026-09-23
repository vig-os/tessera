# Anatomy of a `.tsra`

A product is a **manifest spine** plus **N typed blocks**. The manifest is the small, range-readable
JSON that says *what the product is*; the blocks carry the bytes.

## The three hashes

Every product carries three identifiers, each with a distinct job (ADR-0020):

| Field | What it is | Changes when… |
|---|---|---|
| `id` | logical identity — `blake3(JCS(id_inputs))` | the *logical* thing changes (never on a byte re-encode or rename) |
| `content_hash` | recursive MMR Merkle root over the ordered block digests (ADR-0028) | any block's bytes change |
| `manifest_hash` | the seal — `blake3(JCS(manifest))` | *any* manifest byte changes (metadata, sources, a digest) |

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
