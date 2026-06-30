//! Blob block backend — seal a file's bytes **verbatim** and recover them bit-identical.
//!
//! No codec, no decode: the payload is the file, the digest is `blake3(bytes)`. Because the container's
//! pack/verify path is kind-agnostic (it stores each [`BlockPayload`] uncompressed at `blocks/<name>` and
//! re-hashes on read), a blob needs no special storage path — only this producer, which pairs
//! `blake3(bytes)` with the raw bytes and a self-describing [`BlobSpec`]. `tessera verify` then proves the
//! bytes are unchanged since sealing. The opaque preservation tier behind [`tessera_core::block::blob`].

use std::io::Read;
use std::path::Path;

use tessera_core::block::blob::BlobSpec;
use tessera_core::block::{BlockKind, BlockRef};
use tessera_core::{Error, Result};

use crate::BlockPayload;

/// Chunk size (16 MiB) for [`digest_file_parallel`]'s read → `update_rayon` loop. Sized so each call to
/// `update_rayon` has enough bytes for blake3's internal tree to split work across many cores (blake3
/// parallelizes at a 128 KiB internal granularity, so 16 MiB ≈ 128 sub-chunks → cleanly saturates a
/// 32+ core box), while keeping peak per-call RSS modest (one chunk in RAM at a time).
const PARALLEL_HASH_CHUNK: usize = 16 * 1024 * 1024;

/// Seal raw `bytes` as an opaque blob block named `name`: digest = `blake3(bytes)`, descriptor records
/// `filename` / `media_type` / size. The returned payload is the bytes verbatim (stored uncompressed in
/// the `.tsra`), so [`crate::container::Reader::read_block`] recovers them byte-identical.
pub fn blob_block(
    name: &str,
    filename: &str,
    media_type: Option<&str>,
    bytes: Vec<u8>,
) -> Result<(BlockRef, BlockPayload)> {
    let size =
        u64::try_from(bytes.len()).map_err(|_| Error::Invalid("blob: size exceeds u64".into()))?;
    let mut spec = BlobSpec::new(filename, size);
    if let Some(mt) = media_type {
        spec = spec.with_media_type(mt);
    }
    let digest = tessera_core::hash::digest(&bytes);
    let block_ref = BlockRef {
        name: name.to_string(),
        kind: BlockKind::Blob,
        digest: Some(digest),
        spec: serde_json::to_value(&spec)?,
    };
    Ok((block_ref, BlockPayload::new(name, bytes)))
}

/// Build a blob [`BlockRef`] by **streaming** the file at `path` through blake3 in bounded memory — the
/// bytes are never loaded into a `Vec`. Pair it with [`crate::pack_streaming`] handing the **same `path`**
/// as the block's fragment (`(name, path)`), so a multi-GB file seals with bounded RSS. The resulting
/// `BlockRef` + digest are byte-identical to [`blob_block`] over the same bytes (blake3 is incremental).
///
/// **The source file MUST be stable for the duration of the seal.** This hashes the file here, then
/// the packer re-reads it to copy the bytes — a concurrent writer / atomic-replace between the two
/// reads would otherwise seal a `digest` that doesn't match the packed bytes. The ingest engine pairs
/// this builder with [`crate::pack_streaming_verified`], which re-hashes each fragment **as it copies**
/// and returns [`Error::Integrity`] (`what = "block_payload"`) on a mismatch — so the race is caught at
/// pack time, not silently produced and only discovered on the first read. Ingest a quiescent file (or
/// snapshot it first) to avoid that error path entirely.
pub fn blob_ref_streaming(
    name: &str,
    filename: &str,
    media_type: Option<&str>,
    path: &std::path::Path,
) -> Result<BlockRef> {
    let file = std::fs::File::open(path).map_err(Error::from)?;
    let size = file.metadata().map_err(Error::from)?.len();
    let digest =
        tessera_core::hash::digest_reader(std::io::BufReader::new(file)).map_err(Error::from)?;
    let mut spec = BlobSpec::new(filename, size);
    if let Some(mt) = media_type {
        spec = spec.with_media_type(mt);
    }
    Ok(BlockRef {
        name: name.to_string(),
        kind: BlockKind::Blob,
        digest: Some(digest),
        spec: serde_json::to_value(&spec)?,
    })
}

/// **Multi-core** blake3 of the file at `path` → the same `"blake3:<hex>"` digest as
/// [`tessera_core::hash::digest_reader`] would produce over the same bytes (blake3 guarantees
/// byte-identical output regardless of how the input is sliced or how the internal tree is parallelized).
///
/// The read loop pulls up to [`PARALLEL_HASH_CHUNK`] (16 MiB) into RAM, then calls
/// [`blake3::Hasher::update_rayon`] — which fans the chunk out across the global rayon pool. Peak RSS is
/// one chunk (16 MiB) plus blake3's tiny per-thread state; throughput on a CPU-bound multi-GB hash
/// scales near-linearly with cores until the disk read rate is the floor. The 16 MiB buffer is
/// allocated **per call** (not a reused pool) — irrelevant for one-file blob ingest, but if you ever
/// fan this over thousands of small files, hoist the buffer.
///
/// **Opt-in**, host-only (this crate is not in the wasm-core build graph — see the crate's `Cargo.toml`
/// note on the `rayon` feature). The default ingest path stays [`blob_ref_streaming`]'s single-threaded
/// `digest_reader` — callers that *want* the multi-core hash call this directly or
/// [`blob_ref_streaming_parallel`]. Identical digest, just faster.
pub fn digest_file_parallel(path: &Path) -> Result<String> {
    let file = std::fs::File::open(path).map_err(Error::from)?;
    let mut reader = std::io::BufReader::with_capacity(PARALLEL_HASH_CHUNK, file);
    let mut hasher = blake3::Hasher::new();
    let mut buf = vec![0u8; PARALLEL_HASH_CHUNK];
    loop {
        // Fill `buf` with one full chunk's worth of bytes (or until EOF) before the parallel `update_rayon`
        // call — under-filled chunks would just collapse to a single-thread path inside blake3, so we'd
        // rather pay one extra `Read` than fragment the parallelism.
        let mut filled = 0usize;
        while filled < buf.len() {
            let n = reader.read(&mut buf[filled..]).map_err(Error::from)?;
            if n == 0 {
                break;
            }
            filled += n;
        }
        if filled == 0 {
            break;
        }
        hasher.update_rayon(&buf[..filled]);
        if filled < buf.len() {
            break;
        }
    }
    Ok(format!("blake3:{}", hasher.finalize().to_hex()))
}

/// Parallel-hash variant of [`blob_ref_streaming`]: hashes the file at `path` via
/// [`digest_file_parallel`] (multi-core blake3), produces a [`BlockRef`] **byte-identical** to the
/// single-threaded path. Pair it with [`crate::pack_streaming`] handing the **same `path`** as the
/// block's fragment, same as the serial variant.
///
/// Use when: hashing a large (≫ 100 MiB) file on a multi-core box and the disk can keep up with parallel
/// blake3 (i.e. NVMe/cached). For small files, slow storage, or the wasm build, stick with
/// [`blob_ref_streaming`] — the parallel path adds a 16 MiB RAM bump and some rayon scheduling overhead
/// that doesn't pay back below a few hundred MiB of input.
///
/// Same quiescence contract as [`blob_ref_streaming`]: the file MUST be stable from this hash through
/// the subsequent `pack_streaming` re-read, or the sealed digest will not match the packed bytes.
pub fn blob_ref_streaming_parallel(
    name: &str,
    filename: &str,
    media_type: Option<&str>,
    path: &Path,
) -> Result<BlockRef> {
    let size = std::fs::metadata(path).map_err(Error::from)?.len();
    let digest = digest_file_parallel(path)?;
    let mut spec = BlobSpec::new(filename, size);
    if let Some(mt) = media_type {
        spec = spec.with_media_type(mt);
    }
    Ok(BlockRef {
        name: name.to_string(),
        kind: BlockKind::Blob,
        digest: Some(digest),
        spec: serde_json::to_value(&spec)?,
    })
}

/// Parse a blob block's [`BlobSpec`] descriptor out of its manifest [`BlockRef`] (e.g. to recover the
/// original filename for `extract`). Errors if `r` is not a blob block or its spec is malformed.
pub fn spec_of(r: &BlockRef) -> Result<BlobSpec> {
    if r.kind != BlockKind::Blob {
        return Err(Error::Invalid(format!(
            "block '{}' is {:?}, not a blob",
            r.name, r.kind
        )));
    }
    serde_json::from_value(r.spec.clone()).map_err(|e| Error::Invalid(format!("blob spec: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::container::{pack, Reader};
    use tessera_core::ProductBuilder;

    /// Round-trip: an arbitrary opaque byte blob seals into a `.tsra` and `read_block` returns it
    /// **byte-identical**, and the recorded digest is `blake3(bytes)` (so `verify` confirms it).
    #[test]
    fn blob_round_trips_byte_identical_and_digest_is_blake3() {
        let dir = tempfile::tempdir().unwrap();
        // "junk" bytes that no codec/parser would touch — high entropy + embedded NULs.
        let bytes: Vec<u8> = (0..40_000u32)
            .map(|k| (k.wrapping_mul(2654435761) >> 13) as u8)
            .collect();
        let (bref, payload) = blob_block(
            "data",
            "testscan.l64",
            Some("application/octet-stream"),
            bytes.clone(),
        )
        .unwrap();
        assert_eq!(
            bref.digest.as_deref(),
            Some(tessera_core::hash::digest(&bytes).as_str())
        );

        let mut b = ProductBuilder::new("blob", "DP", "opaque file", "2024-01-01T00:00:00Z");
        b.add_block_ref(bref);
        let sealed = b.seal().unwrap();
        let out = dir.path().join("junk.tsra");
        pack(&sealed, &[payload], &out).unwrap();

        // verify (re-hash every block) passes, then the bytes come back identical.
        let mut reader = Reader::open(&out).unwrap();
        let got = reader.read_block("data").unwrap();
        assert_eq!(
            got, bytes,
            "extracted blob bytes must be byte-identical to the input"
        );

        // the descriptor round-trips (filename preserved for `extract`).
        let spec = spec_of(&reader.manifest().blocks[0]).unwrap();
        assert_eq!(spec.filename, "testscan.l64");
        assert_eq!(spec.size, bytes.len() as u64);
    }

    /// **Determinism gate** for the opt-in multi-core hash path: hashing the same file with the
    /// single-threaded [`tessera_core::hash::digest_reader`] / [`blob_ref_streaming`] and the parallel
    /// [`digest_file_parallel`] / [`blob_ref_streaming_parallel`] MUST produce a byte-identical
    /// `"blake3:<hex>"` digest. blake3 guarantees this regardless of how the input is sliced or how its
    /// internal tree is parallelized — this test pins that guarantee at our API boundary so a future
    /// refactor (chunk size, reader wrapper, etc.) can't silently fork the content-addressed format.
    ///
    /// The fixture is sized to cross [`PARALLEL_HASH_CHUNK`] (16 MiB) by ~2.5x so the parallel path
    /// actually exercises the multi-chunk + rayon split — a sub-chunk file would degenerate to a single
    /// hash call and miss the load-bearing case.
    #[test]
    fn parallel_hash_equals_single_threaded_hash_byte_for_byte() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big.bin");
        // ~40 MiB of pseudo-random bytes: spans 2 full 16 MiB chunks + a partial tail, so the parallel
        // loop pays the full update_rayon → update_rayon → update_rayon dance the real-world large-blob
        // path takes.
        let size: usize = (PARALLEL_HASH_CHUNK * 5) / 2;
        let mut bytes = vec![0u8; size];
        for (i, b) in bytes.iter_mut().enumerate() {
            *b = ((i as u32).wrapping_mul(2_654_435_761) >> 13) as u8;
        }
        std::fs::write(&path, &bytes).unwrap();

        // 1. raw digest helpers: parallel == single-thread streamed == one-shot.
        let serial = tessera_core::hash::digest_reader(std::io::BufReader::new(
            std::fs::File::open(&path).unwrap(),
        ))
        .unwrap();
        let parallel = digest_file_parallel(&path).unwrap();
        let one_shot = tessera_core::hash::digest(&bytes);
        assert_eq!(
            serial, parallel,
            "parallel blake3 MUST be byte-identical to the single-thread digest"
        );
        assert_eq!(
            parallel, one_shot,
            "parallel blake3 MUST also match the one-shot digest over the full buffer"
        );
        assert!(parallel.starts_with("blake3:"));

        // 2. and the wrapped [`BlockRef`] producers agree on name/digest/spec.size — the seal-level
        //    invariant a downstream `verify` will check.
        let serial_ref =
            blob_ref_streaming("data", "big.bin", Some("application/octet-stream"), &path).unwrap();
        let parallel_ref =
            blob_ref_streaming_parallel("data", "big.bin", Some("application/octet-stream"), &path)
                .unwrap();
        assert_eq!(serial_ref.name, parallel_ref.name);
        assert_eq!(serial_ref.kind, parallel_ref.kind);
        assert_eq!(serial_ref.digest, parallel_ref.digest);
        assert_eq!(serial_ref.spec, parallel_ref.spec);
    }

    /// Edge case: an EMPTY file hashes to the empty-input blake3 on both paths. Guards against a chunk
    /// loop that "forgets" to call `finalize` on a zero-byte input.
    #[test]
    fn parallel_hash_handles_empty_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("empty.bin");
        std::fs::write(&path, b"").unwrap();
        let parallel = digest_file_parallel(&path).unwrap();
        let one_shot = tessera_core::hash::digest(b"");
        assert_eq!(parallel, one_shot);
    }

    /// Edge case: a file SMALLER than the parallel chunk (here 1 MiB vs 16 MiB) still produces the same
    /// digest as the single-thread path — confirms the early-EOF branch of the read loop is correct.
    #[test]
    fn parallel_hash_handles_sub_chunk_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("small.bin");
        let bytes: Vec<u8> = (0..1024 * 1024u32).map(|k| (k & 0xff) as u8).collect();
        std::fs::write(&path, &bytes).unwrap();
        let parallel = digest_file_parallel(&path).unwrap();
        let serial = tessera_core::hash::digest(&bytes);
        assert_eq!(parallel, serial);
    }
}
