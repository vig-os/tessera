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
/// `source_label` overrides the recorded `ingested_from` reference (ADR-0040 PHI hygiene — keeps absolute
/// PHI-bearing paths out of the sealed manifest); `None` records the path as before.
///
/// **Memory:** reads the whole file into RAM (peak RSS ≈ file size). The in-memory convenience for when
/// you already hold/produce the bytes; for a large file on disk prefer [`to_blob_product_streaming`]
/// (what the ingest engine uses) — bounded memory, byte-identical sealed result.
/// The `filename` recorded in a blob's [`tessera_io::blob::BlobSpec`]: the `source_label` when given
/// (PHI hygiene — #269; directory separators collapsed so it stays a single valid filename for
/// `extract`), else the input path's own filename (legacy behavior; missing → `"blob"`).
fn blob_filename(path: &Path, source_label: Option<&str>) -> String {
    match source_label {
        Some(label) => label.replace(['/', '\\'], "_"),
        None => path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("blob")
            .to_string(),
    }
}

pub fn to_blob_product(
    path: &Path,
    name: &str,
    timestamp: &str,
    media_type: Option<&str>,
    source_label: Option<&str>,
    extra_sources: &[tessera_core::provenance::Source],
) -> Result<(Manifest, Vec<BlockPayload>)> {
    let bytes = std::fs::read(path).map_err(Error::from)?;
    // PHI hygiene (#269): when a `source_label` is given, it also becomes the blob's stored
    // `filename` — a vendor filename (`PATIENT_SMITH.l64`) is itself PHI and would otherwise ride in
    // the sealed `BlobSpec.filename`, a side-channel the redacted `ingested_from` reference wouldn't
    // catch. Without a label, the real filename is kept (the legacy behavior, needed by `extract`).
    let filename = blob_filename(path, source_label);
    let (block_ref, payload) = blob_block("data", &filename, media_type, bytes)?;
    // The blob block's digest already IS blake3(file bytes) — reuse it as the source-of-record hash
    // on the `ingested_from` edge (no second read of a possibly-multi-GB file).
    let src_digest = block_ref.digest.clone();
    let mut b = ProductBuilder::new("blob", name, "opaque preserved file", timestamp);
    b.add_block_ref(block_ref);
    let source_ref = source_label
        .map(str::to_string)
        .unwrap_or_else(|| path.display().to_string());
    b.add_source(match src_digest {
        Some(d) => crate::provenance::ingested_from_digest(source_ref, d),
        None => crate::provenance::ingested_from(&[path], source_ref)?,
    });
    for s in extra_sources {
        b.add_source(s.clone());
    }
    let sealed = b.seal()?;
    Ok((sealed, vec![payload]))
}

/// Like [`to_blob_product`] but **bounded-memory**: streams the file through blake3 (no whole-file
/// `Vec`) and returns only the sealed manifest — the caller seals it with [`tessera_io::pack_streaming`]
/// handing the **same `path`** as the `data` block's fragment, so a multi-GB file never enters RAM. The
/// sealed bytes + digest are identical to [`to_blob_product`] (blake3 + `pack_streaming` are exact).
///
/// `source_label` overrides the recorded `ingested_from` reference (PHI hygiene); `None` records the path.
pub fn to_blob_product_streaming(
    path: &Path,
    name: &str,
    timestamp: &str,
    media_type: Option<&str>,
    source_label: Option<&str>,
    extra_sources: &[tessera_core::provenance::Source],
) -> Result<Manifest> {
    let filename = blob_filename(path, source_label);
    let block_ref = tessera_io::blob::blob_ref_streaming("data", &filename, media_type, path)?;
    // The streamed block digest already IS blake3(file bytes) — reuse it, no second pass.
    let src_digest = block_ref.digest.clone();
    let mut b = ProductBuilder::new("blob", name, "opaque preserved file", timestamp);
    b.add_block_ref(block_ref);
    let source_ref = source_label
        .map(str::to_string)
        .unwrap_or_else(|| path.display().to_string());
    b.add_source(match src_digest {
        Some(d) => crate::provenance::ingested_from_digest(source_ref, d),
        None => crate::provenance::ingested_from(&[path], source_ref)?,
    });
    for s in extra_sources {
        b.add_source(s.clone());
    }
    b.seal()
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

        let (m, payloads) = to_blob_product(
            &src,
            "KSB-testscan",
            "2024-01-01T00:00:00Z",
            None,
            None,
            &[],
        )
        .unwrap();
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

    /// ADR-0040: when `source_label` is given, the recorded `ingested_from` reference is the LABEL,
    /// not the (potentially PHI-bearing) path. Proves the seam reaches the manifest on BOTH
    /// constructors (the streaming variant is what the engine drives).
    #[test]
    fn source_label_overrides_ingested_from_reference() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("scan.l64");
        std::fs::write(&src, b"some opaque bytes").unwrap();

        let (m, _) = to_blob_product(
            &src,
            "x",
            "2024-01-01T00:00:00Z",
            None,
            Some("DUPLET-07/raw"),
            &[],
        )
        .unwrap();
        let ingested_from = m
            .sources
            .iter()
            .find(|s| s.role == "ingested_from")
            .expect("ingested_from edge");
        assert_eq!(ingested_from.reference, "DUPLET-07/raw");
        assert!(
            !ingested_from.reference.contains(src.to_str().unwrap()),
            "label MUST replace the PHI-bearing path, not be appended to it"
        );

        let m2 = to_blob_product_streaming(
            &src,
            "x",
            "2024-01-01T00:00:00Z",
            None,
            Some("DUPLET-07/raw"),
            &[],
        )
        .unwrap();
        assert_eq!(
            m2.sources
                .iter()
                .find(|s| s.role == "ingested_from")
                .map(|s| s.reference.as_str()),
            Some("DUPLET-07/raw"),
        );

        // #269: the label ALSO redacts the sealed `BlobSpec.filename` — the vendor filename
        // (`scan.l64`) is itself PHI and must not survive as a side-channel. Directory separators in
        // the label are collapsed so it stays a valid single filename for `extract`.
        for man in [&m, &m2] {
            let spec = tessera_io::blob::spec_of(&man.blocks[0]).unwrap();
            assert_eq!(spec.filename, "DUPLET-07_raw");
            assert!(
                !spec.filename.contains("scan.l64"),
                "PHI-bearing filename leaked into the sealed BlobSpec"
            );
        }
    }

    #[test]
    fn streaming_seal_is_identical_to_in_ram() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("scan.l64");
        let bytes: Vec<u8> = (0..70_000u32)
            .map(|k| (k.wrapping_mul(2_654_435_761) >> 9) as u8)
            .collect();
        std::fs::write(&src, &bytes).unwrap();

        let (in_ram, payloads) =
            to_blob_product(&src, "x", "2024-01-01T00:00:00Z", None, None, &[]).unwrap();
        let streamed =
            to_blob_product_streaming(&src, "x", "2024-01-01T00:00:00Z", None, None, &[]).unwrap();
        // bounded-memory streaming yields the SAME identity, content hash, and seal as the in-RAM path.
        assert_eq!(in_ram.id, streamed.id);
        assert_eq!(in_ram.content_hash, streamed.content_hash);
        assert_eq!(in_ram.manifest_hash, streamed.manifest_hash);
        assert_eq!(in_ram.blocks[0].digest, streamed.blocks[0].digest);

        // …and the packed `.tsra` is byte-identical end to end: in-RAM `pack` vs `pack_streaming` with
        // the source file as the `data` fragment (closes the inductive gap to the container layer).
        let ram_tsra = dir.path().join("ram.tsra");
        let stream_tsra = dir.path().join("stream.tsra");
        tessera_io::pack(&in_ram, &payloads, &ram_tsra).unwrap();
        tessera_io::pack_streaming(
            &streamed,
            &[("data".to_string(), src.as_path())],
            &stream_tsra,
        )
        .unwrap();
        assert_eq!(
            std::fs::read(&ram_tsra).unwrap(),
            std::fs::read(&stream_tsra).unwrap(),
            "streamed .tsra must be byte-identical to the in-RAM packed .tsra"
        );
    }
}
