//! Shared provenance-edge construction for ingest backends.
//!
//! Every ingest records an `ingested_from` edge. Historically that edge carried only the source
//! *path* (or a `--source-label`) with no `content_hash` — so nothing cryptographically tied the
//! product to the exact source bytes. These helpers close that gap: the edge's `content_hash` is a
//! blake3 **merkle root** over the per-file digests (a single file → the root of one leaf), so an
//! auditor can prove this product descends from *those* bytes even if the paths later change.
//!
//! The hash is computed over the **raw file bytes** and is independent of the `reference` string, so
//! a PHI-scrubbed `--source-label` never weakens the integrity link.

use std::path::Path;

use tessera_core::provenance::Source;
use tessera_core::Result;

/// blake3 merkle root over the raw bytes of `paths` (streamed, bounded memory per file). The same
/// root a caller can recompute from the source-of-record to verify the `ingested_from` edge.
pub fn source_digest(paths: &[&Path]) -> Result<String> {
    let mut digests = Vec::with_capacity(paths.len());
    for p in paths {
        let f = std::fs::File::open(p)?;
        digests.push(tessera_core::hash::digest_reader(std::io::BufReader::new(
            f,
        ))?);
    }
    Ok(tessera_core::hash::merkle_root(&digests))
}

/// The `ingested_from` edge with its `content_hash` set to the source merkle root — the integrity
/// link back to the source-of-record. `reference` is the caller's display string (a `--source-label`
/// if given, else the path); the hash is independent of it.
pub fn ingested_from(paths: &[&Path], reference: impl Into<String>) -> Result<Source> {
    let digest = source_digest(paths)?;
    Ok(Source::new("ingested_from", reference).with_content_hash(digest))
}

/// Like [`ingested_from`] but with a **precomputed** source digest — for backends (DICOM series) that
/// already hashed the files while decoding them, so the bytes are read only once.
pub fn ingested_from_digest(reference: impl Into<String>, digest: String) -> Source {
    Source::new("ingested_from", reference).with_content_hash(digest)
}
