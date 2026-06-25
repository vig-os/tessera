# ADR-0023 — Array block payload: Zarr v3 + pcodec, serialized as one deterministic blob

Status: **Accepted** (2026-06-26) · Relates to: ADR-0020 (identity/hashing), ADR-0022 (`.tsra`
container). Implements the array half of ROADMAP **P3 / S5** (`#203`).

## Context
The format spine (ADR-0020/0022) was frozen with **placeholder** block payloads: an array block's
stored bytes were just `JCS(spec)`. That proved the manifest/identity/container machinery but encoded
no actual samples. The settled architecture (FEATURE-MATRIX §B, the spike) is: **volumes → Zarr v3 /
OME-Zarr, 64³ cubic chunks, pcodec** (lossless; −21% CT / −33% PET vs zstd). We need the real codec,
and it must satisfy two release gates: **bit-exact roundtrip** (incl float NaN/±inf/−0.0/denormal) and
**writer-determinism** (same input → byte-identical payload).

A Zarr array is not a single file — it is a *set* of store keys (`zarr.json` metadata + one object per
chunk). The `.tsra` container (ADR-0022) stores exactly **one payload entry per block** and digests it
as one byte string. So we must decide how a multi-key Zarr store maps onto one block payload.

## Decision
1. **Codec.** Array blocks are encoded with **zarrs 0.23 (Zarr v3)** using the **pcodec** array→bytes
   codec (`pco` 1.0), default configuration. `tessera-core` stays I/O-free; all encoding lives in
   `tessera-io::array`. The `ArraySpec.codec` field selects the codec; `pcodec` is the default and the
   only one implemented today (a `zstd` fallback may be added later without changing this contract).
2. **Payload = one serialized store blob.** The block's Zarr store is serialized into a single
   deterministic byte string: list every store key, **sort** lexicographically, and emit
   `u32 key_len · key · u64 value_len · value` per entry. Decoding rebuilds an in-memory store and
   reads the array back. The blob is a faithful Zarr store, so the opt-in *exploded* form (ADR-0022)
   can later materialize it as real on-disk OME-Zarr without a format change.
3. **Digest.** The block's `BlockRef.digest` is `blake3` over these payload bytes (ADR-0020), supplied
   to the builder via `ProductBuilder::add_block_ref`. The container's invariant — *stored bytes ==
   what the digest covers* — holds by construction.
4. **Dtypes.** The eight pcodec-native dtypes are supported: `int16/32/64`, `uint16/32/64`,
   `float32/64` — every dtype real imaging volumes use. `int8`/`uint8`/`float16` are out of scope for
   the pcodec backend (pcodec is ≥16-bit); a future zstd fallback can cover them.

## Why this over the alternatives
- **vs. a subtree of container entries** (`blocks/<name>/zarr.json`, `…/c/0/0/0`, …, with a chunk-level
  Merkle): more faithful to "chunks born with a hash", but it changes the container/manifest/Reader
  contract (multi-entry blocks) and front-loads the write-engine redesign. Deferred to P3's streaming
  engine + S3 chunk-Merkle, where per-chunk hashing is the actual deliverable. The single-blob form
  ships the codec value (pcodec, cubic chunks, ROI reads) now without touching the frozen container.
- **vs. pcodec-direct (no Zarr):** would forfeit OME-Zarr interop and the mature chunk-grid / ROI
  machinery for a bespoke frame. Not worth it; zarrs gives sharding and `retrieve_array_subset` for
  free.
- **Determinism** is owned by us: sorting keys + fixed framing removes map-order nondeterminism, and
  pcodec + Zarr v3 metadata are themselves timestamp-free and deterministic. Verified by probe and by
  the conformance gate (`corpus_packs_deterministically`).

## Consequences
- The array fixtures in the conformance corpus now carry real payloads; their `content_hash` /
  `manifest_hash` goldens were regenerated (`id` is unchanged — it is logical identity, not data).
- Reading a sub-region (`decode_subset`) decodes only the intersecting chunks — the orthogonal/ROI
  value of cubic chunking. True *cloud* range-reads (backing the Zarr store by the `.tsra` range
  reader) are a later layer (S6/object_store).
- New dep tree (zarrs + pcodec, pure-Rust, `default-features = false, features = ["pcodec"]`); no C
  toolchain added. `Error::Codec` added to the taxonomy for backend failures.
- Table blocks remain spec-JSON stubs until the Vortex backend (S5 part 2); their goldens will
  regenerate then.
