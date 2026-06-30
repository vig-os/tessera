//! Blob block backend — seal a file's bytes **verbatim** and recover them bit-identical.
//!
//! No codec, no decode: the payload is the file, the digest is `blake3(bytes)`. Because the container's
//! pack/verify path is kind-agnostic (it stores each [`BlockPayload`] uncompressed at `blocks/<name>` and
//! re-hashes on read), a blob needs no special storage path — only this producer, which pairs
//! `blake3(bytes)` with the raw bytes and a self-describing [`BlobSpec`]. `tessera verify` then proves the
//! bytes are unchanged since sealing. The opaque preservation tier behind [`tessera_core::block::blob`].

use tessera_core::block::blob::BlobSpec;
use tessera_core::block::{BlockKind, BlockRef};
use tessera_core::{Error, Result};

use crate::BlockPayload;

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
/// `pack_streaming` re-reads it to copy the bytes — a concurrent writer / atomic-replace between the two
/// reads would seal a `digest` that doesn't match the packed bytes (the `.tsra` would then fail its own
/// block check on first read — caught, never silent). Ingest a quiescent file (or snapshot it first).
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
}
