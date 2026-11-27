//! Shared test helpers for the CLI's unit tests. Test-only: compiled under `cfg(test)`.

use std::io::Read;
use std::path::Path;

use tessera_core::canonical;

/// `bytes` and `seal`, a sealed document and its seal member as a door wrote them, are the
/// ADR-0066 stored pair: `bytes` is exactly its own RFC 8785 JCS form with no top-level
/// `manifest_hash` key (rule 1), and `seal` is the 71 bytes `blake3(bytes)` (rules 2 and 3).
#[track_caller]
pub(crate) fn assert_stored_pair(what: &str, bytes: &[u8], seal: &[u8]) {
    let object = canonical::check_canonical(bytes)
        .unwrap_or_else(|e| panic!("{what}: the written document is not its own JCS form: {e}"));
    assert!(
        object.get("manifest_hash").is_none(),
        "{what}: the written document carries its seal in band"
    );
    assert_eq!(
        seal,
        tessera_core::hash::digest(bytes).as_bytes(),
        "{what}: the written seal is not blake3 of the written document"
    );
}

/// The `.tsra` at `path` stores the ADR-0066 pair, read from the raw zip members (not through
/// `tessera_io::Reader`, whose verification is what is under test): entries 1–3 are `mimetype`,
/// `manifest.seal`, `manifest.json`, and the pair passes [`assert_stored_pair`].
#[track_caller]
pub(crate) fn assert_tsra_stored_pair(what: &str, path: &Path) {
    let mut zip = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
    let names: Vec<String> = (0..3.min(zip.len()))
        .map(|i| zip.by_index(i).unwrap().name().unwrap().into_owned())
        .collect();
    assert_eq!(
        names,
        ["mimetype", "manifest.seal", "manifest.json"],
        "{what}: the leading entries"
    );
    let mut member = |name: &str| {
        let mut out = Vec::new();
        zip.by_name(name).unwrap().read_to_end(&mut out).unwrap();
        out
    };
    let seal = member("manifest.seal");
    let bytes = member("manifest.json");
    assert_stored_pair(what, &bytes, &seal);
}

/// Write a deliberately invalid product below the writer's validation gates, for reader tests.
/// The archive layout and stored seal pair stay canonical; the caller supplies the invalid manifest.
pub(crate) fn write_invalid_tsra(
    path: &Path,
    manifest: &tessera_core::Manifest,
    payloads: &[tessera_io::BlockPayload],
) {
    use std::io::Write;
    let (json, seal) = tessera_io::stored::manifest(manifest).unwrap();
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Stored)
        .last_modified_time(zip::DateTime::default())
        .large_file(true)
        .system(zip::System::Unix);
    let mut archive = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
    for (name, bytes) in [
        ("mimetype", tessera_io::MIMETYPE.as_bytes()),
        ("manifest.seal", seal.as_bytes()),
        ("manifest.json", json.as_slice()),
    ] {
        archive.start_file(name, options).unwrap();
        archive.write_all(bytes).unwrap();
    }
    for payload in payloads {
        archive
            .start_file(format!("blocks/{}", payload.name), options)
            .unwrap();
        archive.write_all(&payload.bytes).unwrap();
    }
    archive.finish().unwrap();
}
