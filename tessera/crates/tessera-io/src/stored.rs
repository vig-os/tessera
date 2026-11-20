//! The stored form of a sealed JSON document on disk (ADR-0066 rules 1–5): the one place tessera
//! writes and reads `manifest.json` + `manifest.seal` in the exploded form and `collection.json` +
//! `collection.seal` beside a collection's members.
//!
//! A sealed document is stored as exactly its RFC 8785 JCS bytes (no whitespace, keys in JCS order,
//! JCS escapes and numbers, UTF-8, one line, no `manifest_hash` key), and its seal,
//! `"blake3:" + hex(blake3(bytes))`, is a separate member or file of 71 ASCII bytes beside it.
//! `jq . manifest.json` gives the readable view, and `b3sum manifest.json` checks the seal.
//!
//! Writers serialise a typed value with [`Manifest::to_stored`] / [`Collection::to_stored`], which
//! refuse a view that would not carry its seal (edited after sealing, a parsed view with newer
//! fields). A door that copies a product it read (`unpack`, `pack_dir`, adding `aux/` members)
//! copies its stored bytes and seal verbatim, after reading them through `from_stored_members`.
//! Readers go through [`tessera_core::Manifest::from_stored_members`] /
//! [`tessera_core::Collection::from_stored_members`]: a missing seal is the alpha layout, refused
//! naming both formats (ADR-0066 rule 10, no migration).

use std::path::{Path, PathBuf};

use tessera_core::collection::Collection;
use tessera_core::manifest::Manifest;
use tessera_core::{Error, Result};

/// The stored pair of a sealed manifest: `(manifest.json bytes, manifest.seal)`.
///
/// Used by [`crate::pack`], [`crate::pack_streaming`], [`crate::WriteSession`]'s seal and
/// [`crate::Repository::put_manifest`], so every manifest tessera writes from a typed value is
/// stored in this form.
///
/// # Errors
/// [`Error::Invalid`] for an unsealed manifest; then as [`Manifest::to_stored`]: a parsed view with
/// newer fields, or a manifest whose bytes don't reproduce its seal, is refused.
pub fn manifest(m: &Manifest) -> Result<(Vec<u8>, String)> {
    if !m.is_sealed() {
        return Err(Error::Invalid(
            "refusing to store an unsealed manifest as a product".into(),
        ));
    }
    m.to_stored()
}

/// The stored pair of a sealed collection: `(collection.json bytes, collection.seal)`.
///
/// # Errors
/// As [`manifest`], for a collection.
pub fn collection(c: &Collection) -> Result<(Vec<u8>, String)> {
    if !c.is_sealed() {
        return Err(Error::Invalid(
            "refusing to store an unsealed collection".into(),
        ));
    }
    c.to_stored()
}

/// The path of the seal file beside the document at `document` (`.json` → `.seal`).
///
/// # Errors
/// [`Error::Invalid`] when `document`'s file name does not end in `.json`.
pub fn seal_path(document: &Path) -> Result<PathBuf> {
    document
        .file_name()
        .and_then(|n| n.to_str())
        .and_then(tessera_core::stored::seal_name_for)
        .map(|name| document.with_file_name(name))
        .ok_or_else(|| {
            Error::Invalid(format!(
                "{}: a sealed document's file name ends in `.json`",
                document.display()
            ))
        })
}

/// Write a stored pair: `bytes` to `document`, `seal` to [`seal_path`] beside it.
///
/// # Errors
/// [`Error::Io`] for a write that fails; [`Error::Invalid`] as [`seal_path`].
pub fn write_pair(document: &Path, bytes: &[u8], seal: &str) -> Result<()> {
    let seal_file = seal_path(document)?;
    std::fs::write(document, bytes)?;
    std::fs::write(seal_file, seal.as_bytes())?;
    Ok(())
}

/// Read a stored pair from disk: the document's bytes and its seal file's bytes (`None` when the
/// seal file does not exist, which a reader refuses as the alpha layout).
///
/// # Errors
/// [`Error::Io`] when the document can't be read, or the seal file exists and can't be read.
pub fn read_pair(document: &Path) -> Result<(Vec<u8>, Option<Vec<u8>>)> {
    let bytes = std::fs::read(document)?;
    let seal = match std::fs::read(seal_path(document)?) {
        Ok(s) => Some(s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.into()),
    };
    Ok((bytes, seal))
}

/// Write a sealed collection as `document` (`collection.json`, or `<stem>.collection.json` for a
/// sub-collection member) and its seal beside it.
///
/// # Errors
/// As [`collection`] and [`write_pair`].
pub fn write_collection(document: &Path, c: &Collection) -> Result<()> {
    let (bytes, seal) = collection(c)?;
    write_pair(document, &bytes, &seal)
}

/// Read and verify the collection stored at `document` and its seal beside it
/// ([`Collection::from_stored_members`]).
///
/// # Errors
/// As [`read_pair`], then everything [`Collection::from_stored_members`] refuses, including the
/// alpha layout (no seal file).
pub fn read_collection(document: &Path) -> Result<Collection> {
    let (bytes, seal) = read_pair(document)?;
    Collection::from_stored_members(&bytes, seal.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tessera_core::CollectionBuilder;

    #[test]
    fn seal_files_sit_beside_their_documents() {
        assert_eq!(
            seal_path(Path::new("out/collection.json")).unwrap(),
            Path::new("out/collection.seal")
        );
        assert_eq!(
            seal_path(Path::new("out/blake3_ab.collection.json")).unwrap(),
            Path::new("out/blake3_ab.collection.seal")
        );
        assert!(seal_path(Path::new("out/collection")).is_err());
    }

    /// The collection writer's door: the written seal is blake3 of the written bytes, the pair
    /// reads back, and a missing or mismatched seal file is refused.
    #[test]
    fn a_written_collection_seal_is_the_digest_of_the_written_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let doc = dir.path().join("collection.json");
        let c = CollectionBuilder::new("c", "d", "2024-01-01T00:00:00Z")
            .seal()
            .unwrap();
        write_collection(&doc, &c).unwrap();
        let bytes = std::fs::read(&doc).unwrap();
        let seal = std::fs::read_to_string(dir.path().join("collection.seal")).unwrap();
        assert_eq!(seal, tessera_core::hash::digest(&bytes));
        assert_eq!(
            tessera_core::canonical::check_canonical(&bytes)
                .unwrap()
                .get("manifest_hash"),
            None
        );
        assert_eq!(read_collection(&doc).unwrap(), c);

        std::fs::write(
            dir.path().join("collection.seal"),
            format!("blake3:{}", "0".repeat(64)),
        )
        .unwrap();
        assert!(matches!(
            read_collection(&doc),
            Err(Error::Integrity { .. })
        ));
        std::fs::remove_file(dir.path().join("collection.seal")).unwrap();
        assert!(matches!(
            read_collection(&doc),
            Err(Error::UnsupportedLayout { .. })
        ));
    }

    #[test]
    fn an_unsealed_document_is_not_stored() {
        let c = Collection::new("c", "d", "2024-01-01T00:00:00Z");
        assert!(matches!(collection(&c), Err(Error::Invalid(_))));
    }
}
