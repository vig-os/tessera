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

/// The path of the seal file beside the document at `document` (`.json` → `.seal`:
/// `manifest.json` → `manifest.seal`, `<stem>.collection.json` → `<stem>.collection.seal`).
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

/// Write a stored pair: `bytes` to `document`, `seal` to [`seal_path`] beside it. The caller
/// passes a pair it sealed or verified ([`Manifest::to_stored`], [`Collection::to_stored`]); this
/// only places it.
///
/// **Crash-consistent.** Each file is written durably and atomically (a temp file beside it,
/// fsync, rename, fsync of the directory), the document first and the seal last, so neither file
/// is ever torn. A crash between the two renames leaves the new document beside the previous seal,
/// which a reader refuses as a seal mismatch ([`Error::Integrity`]), or, when there was no previous
/// seal, beside none, which a reader refuses as the alpha layout ([`Error::UnsupportedLayout`]).
/// Neither verifies, so a half-written pair is never read as sealed; writing the pair again
/// completes it. A crash can leave a `<name>.tmp` file, which the next write replaces.
///
/// # Errors
/// [`Error::Io`] for a write that fails; [`Error::Invalid`] as [`seal_path`].
pub fn write_pair(document: &Path, bytes: &[u8], seal: &str) -> Result<()> {
    let seal_file = seal_path(document)?;
    crate::write::write_durable(document, bytes)?;
    crate::write::write_durable(&seal_file, seal.as_bytes())?;
    Ok(())
}

/// Read a stored pair from disk: the document's bytes and its seal file's first
/// [`SEAL_LEN`](tessera_core::stored::SEAL_LEN)` + 1` bytes (`None` when the seal file does not
/// exist, which [`Manifest::from_stored_members`] /
/// [`Collection::from_stored_members`] refuse as the alpha layout). Nothing is verified here.
///
/// # Errors
/// [`Error::Io`] when the document can't be read, or the seal file exists and can't be read;
/// [`Error::Invalid`] as [`seal_path`].
pub fn read_pair(document: &Path) -> Result<(Vec<u8>, Option<Vec<u8>>)> {
    use std::io::Read;
    let bytes = std::fs::read(document)?;
    let seal = match std::fs::File::open(seal_path(document)?) {
        Ok(f) => {
            // At most one byte past a seal's length, as `Reader` reads `manifest.seal`: a longer
            // file is malformed whatever follows, and is never read in full.
            let mut s = Vec::with_capacity(tessera_core::stored::SEAL_LEN + 1);
            f.take(tessera_core::stored::SEAL_LEN as u64 + 1)
                .read_to_end(&mut s)?;
            Some(s)
        }
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
/// ([`collection_from_pair`]).
///
/// # Errors
/// As [`read_pair`], then everything [`Collection::from_stored_members`] refuses, including the
/// alpha layout (no seal file).
pub fn read_collection(document: &Path) -> Result<Collection> {
    let (bytes, seal) = read_pair(document)?;
    collection_from_pair(document, &bytes, seal.as_deref())
}

/// Verify a collection pair [`read_pair`] read from `document`
/// ([`Collection::from_stored_members_as`]), so a refusal names the files as they are on disk
/// (`<stem>.collection.json` and `<stem>.collection.seal` for a sub-collection member).
///
/// # Errors
/// [`Error::Invalid`] as [`seal_path`]; then everything [`Collection::from_stored_members`]
/// refuses.
pub fn collection_from_pair(
    document: &Path,
    bytes: &[u8],
    seal: Option<&[u8]>,
) -> Result<Collection> {
    let name = document
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    Collection::from_stored_members_as(name, bytes, seal)
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
        assert_eq!(
            seal_path(Path::new("dir/manifest.json")).unwrap(),
            Path::new("dir/manifest.seal")
        );
        assert!(matches!(
            seal_path(Path::new("out/collection")),
            Err(Error::Invalid(_))
        ));
    }

    /// A pair written with `write_pair` reads back with `read_pair` and verifies; the written seal
    /// file is exactly the seal (71 bytes, no newline); without the seal file the document is the
    /// alpha layout, and with another seal it is a seal mismatch.
    #[test]
    fn a_written_pair_reads_back_and_a_missing_or_wrong_seal_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let doc = dir.path().join("collection.json");
        let c = tessera_core::CollectionBuilder::new("c", "d", "2024-01-01T00:00:00Z")
            .seal()
            .unwrap();
        let (bytes, seal) = c.to_stored().unwrap();
        write_pair(&doc, &bytes, &seal).unwrap();
        assert_eq!(std::fs::read(&doc).unwrap(), bytes);
        let seal_file = dir.path().join("collection.seal");
        assert_eq!(std::fs::read(&seal_file).unwrap(), seal.as_bytes());
        assert_eq!(seal.len(), tessera_core::stored::SEAL_LEN);

        let (got, got_seal) = read_pair(&doc).unwrap();
        assert_eq!(
            (got.as_slice(), got_seal.as_deref()),
            (bytes.as_slice(), Some(seal.as_bytes()))
        );
        assert_eq!(
            Collection::from_stored_members(&got, got_seal.as_deref()).unwrap(),
            c
        );

        std::fs::write(&seal_file, format!("blake3:{}", "0".repeat(64))).unwrap();
        let (got, got_seal) = read_pair(&doc).unwrap();
        assert!(matches!(
            Collection::from_stored_members(&got, got_seal.as_deref()),
            Err(Error::Integrity { .. })
        ));
        std::fs::remove_file(&seal_file).unwrap();
        let (got, got_seal) = read_pair(&doc).unwrap();
        assert_eq!(got_seal, None);
        assert!(matches!(
            Collection::from_stored_members(&got, None),
            Err(Error::UnsupportedLayout { .. })
        ));
        assert!(matches!(
            read_pair(&dir.path().join("absent.json")),
            Err(Error::Io(_))
        ));
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

    /// #939 item 5: a crash between the document's rename and the seal's leaves a pair no reader
    /// verifies (the new document beside the old seal is a seal mismatch; with no old seal it is the
    /// alpha layout), and the old pair is intact while the document's own rename is pending.
    #[test]
    fn a_crash_inside_write_pair_never_leaves_a_pair_that_verifies() {
        use crate::write::seam;
        let dir = tempfile::tempdir().unwrap();
        let doc = dir.path().join("collection.json");
        let stored = |title: &str| {
            tessera_core::CollectionBuilder::new("c", title, "2024-01-01T00:00:00Z")
                .seal()
                .unwrap()
                .to_stored()
                .unwrap()
        };
        let read = || {
            let (bytes, seal) = read_pair(&doc).unwrap();
            Collection::from_stored_members(&bytes, seal.as_deref())
        };
        let write_crashing_at = |nth: usize, (bytes, seal): &(Vec<u8>, String)| {
            let _crash = seam::arm(&seam::FAIL_NTH_DURABLE_RENAME, nth);
            assert!(write_pair(&doc, bytes, seal).is_err());
        };
        let (old, new) = (stored("old"), stored("new"));

        // First write ever, crash before the seal's rename: the document without a seal.
        write_crashing_at(2, &new);
        assert_eq!(std::fs::read(&doc).unwrap(), new.0);
        assert!(matches!(read(), Err(Error::UnsupportedLayout { .. })));

        // Over an existing pair: a crash before the document's rename keeps the old pair whole …
        write_pair(&doc, &old.0, &old.1).unwrap();
        write_crashing_at(1, &new);
        assert_eq!(
            read().unwrap().manifest_hash.as_deref(),
            Some(old.1.as_str())
        );
        // … and a crash before the seal's rename is a seal mismatch, neither old nor new.
        write_crashing_at(2, &new);
        assert!(matches!(read(), Err(Error::Integrity { .. })));
        // Writing the pair again completes it.
        write_pair(&doc, &new.0, &new.1).unwrap();
        assert_eq!(
            read().unwrap().manifest_hash.as_deref(),
            Some(new.1.as_str())
        );
    }

    /// A seal file longer than a seal is read only one byte past [`SEAL_LEN`] and refused as
    /// malformed, naming the file beside the document.
    #[test]
    fn an_oversized_seal_file_is_read_bounded_and_refused() {
        use tessera_core::stored::SEAL_LEN;
        let dir = tempfile::tempdir().unwrap();
        let doc = dir.path().join("blake3_ab.collection.json");
        let (bytes, seal) = CollectionBuilder::new("c", "d", "2024-01-01T00:00:00Z")
            .seal()
            .unwrap()
            .to_stored()
            .unwrap();
        write_pair(&doc, &bytes, &seal).unwrap();
        std::fs::write(seal_path(&doc).unwrap(), seal.repeat(1000)).unwrap();
        let (_, got) = read_pair(&doc).unwrap();
        assert_eq!(got.as_deref().map(<[u8]>::len), Some(SEAL_LEN + 1));
        assert!(matches!(
            read_collection(&doc),
            Err(Error::MalformedSeal { member, .. }) if member == "blake3_ab.collection.seal"
        ));
    }

    #[test]
    fn an_unsealed_document_is_not_stored() {
        let c = Collection::new("c", "d", "2024-01-01T00:00:00Z");
        assert!(matches!(collection(&c), Err(Error::Invalid(_))));
    }
}
