//! Blob ("junk") ingest backend — wrap an un-parsed vendor file as an opaque, bit-faithful `blob`
//! product. The escape hatch for the long tail the typed backends don't cover (Siemens `.l64`, GE
//! `.7z`/`.cal`, PDFs, logs): read the file whole, seal its bytes verbatim with a `blake3` digest, and
//! record an `ingested_from` provenance edge. See [`tessera_core::block::blob`].

use std::path::Path;

use tessera_core::manifest::Manifest;
use tessera_core::{Error, ProductBuilder, Result};
use tessera_io::blob::blob_block;
use tessera_io::BlockPayload;

/// Read `path` whole and seal it as a single-blob `blob` product. The block is named `data`; the source
/// basename is preserved in the descriptor (the default `tessera extract` name). `extra_sources` flow in
/// after the `ingested_from` edge (the declarative engine threads `derived_from` / `ingested_via_spec`).
///
/// **Memory:** reads the whole file into RAM (peak RSS ≈ file size) — fine for the common cases;
/// bounded-memory streaming for multi-GB `.l64` is the tracked follow-up (#231). The sealed bytes +
/// digest are identical either way.
pub fn to_blob_product(
    path: &Path,
    name: &str,
    timestamp: &str,
    media_type: Option<&str>,
    extra_sources: &[tessera_core::provenance::Source],
) -> Result<(Manifest, Vec<BlockPayload>)> {
    let bytes = std::fs::read(path).map_err(Error::from)?;
    let filename = path.file_name().and_then(|s| s.to_str()).unwrap_or("blob");
    let (block_ref, payload) = blob_block("data", filename, media_type, bytes)?;
    let mut b = ProductBuilder::new("blob", name, "opaque preserved file", timestamp);
    b.add_block_ref(block_ref);
    b.add_source(tessera_core::provenance::Source::new(
        "ingested_from",
        path.display().to_string(),
    ));
    for s in extra_sources {
        b.add_source(s.clone());
    }
    let sealed = b.seal()?;
    Ok((sealed, vec![payload]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seals_an_arbitrary_file_as_a_verifiable_blob_product() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("testscan.l64");
        let bytes: Vec<u8> = (0..5_000u32).map(|k| (k % 256) as u8).collect();
        std::fs::write(&src, &bytes).unwrap();

        let (m, payloads) =
            to_blob_product(&src, "KSB-testscan", "2024-01-01T00:00:00Z", None, &[]).unwrap();
        assert_eq!(m.product, "blob");
        assert_eq!(m.blocks.len(), 1);
        assert_eq!(
            m.blocks[0].digest.as_deref(),
            Some(tessera_core::hash::digest(&bytes).as_str())
        );
        assert_eq!(payloads[0].bytes, bytes);
        // provenance: the source file is recorded.
        assert!(m
            .sources
            .iter()
            .any(|s| s.reference.contains("testscan.l64")));
    }
}
