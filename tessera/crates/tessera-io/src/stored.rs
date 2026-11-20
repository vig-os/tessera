//! The stored form of a sealed JSON document: the bytes every writer puts on disk for
//! `manifest.json` and `collection.json` (ADR-0066 decision rule 1, implementation step 2).
//!
//! A sealed document is stored as exactly its RFC 8785 JCS serialisation: no whitespace, keys in
//! JCS order, JCS string escapes and number forms, UTF-8, one line. `jq . manifest.json` gives the
//! readable view. Until the A′ cutover (ADR-0066 step 4) the stored bytes still carry the in-band
//! `manifest_hash` key, and readers still accept any JSON spelling of a sealed object; this module
//! only fixes what tessera's own writers emit.
//!
//! **The seal does not move.** The seal is `digest(JCS(object − "manifest_hash"))` over the object
//! a reader parses (SPEC §4). Every function here re-encodes a *parsed* value with
//! [`tessera_core::canonical::to_bytes`] (the one JCS producer), and parsing JCS output gives back
//! the same object: a JCS number is the shortest form that round-trips its double, and a JCS key or
//! string unescapes to the same text. So the object a reader parses from these bytes is the object
//! parsed from the writer's previous output, key for key and number for number, and it seals to the
//! same `manifest_hash`. The corpus proof is `tests/stored_form.rs`.
//!
//! One path keeps the stored bytes instead: adding `aux/` members to an existing `.tsra`
//! ([`crate::add_aux_members`], used by signing and ingest provenance) copies `manifest.json`
//! byte-for-byte and refuses to change it, because aux members are outside the seal. A product
//! written before #912 therefore keeps its pretty manifest through `sign`; re-packing it
//! (`tessera unpack` + `tessera pack`) stores the JCS form under the same seal.
//!
//! I-JSON (RFC 7493, ADR-0066 rule 7) stays enforced at every door: the input is parsed with
//! [`tessera_core::canonical::parse_strict`] (duplicate keys, lone surrogates, integers beyond
//! ±(2^53−1)) and [`tessera_core::canonical::to_bytes`] checks the integers again.

use std::path::{Path, PathBuf};

use tessera_core::canonical;
use tessera_core::collection::Collection;
use tessera_core::manifest::Manifest;
use tessera_core::{Error, Result};

/// The stored bytes of a sealed manifest: the JCS form of [`Manifest::to_sealed_json`].
///
/// Used by [`crate::pack`], [`crate::pack_streaming`], [`crate::WriteSession`]'s seal and
/// [`crate::Repository::put_manifest`], so every manifest tessera writes from a typed value is
/// stored in this form.
///
/// # Errors
/// As [`Manifest::to_sealed_json`]: an unsealed manifest, a parsed view with newer fields, or a
/// manifest whose JSON doesn't reproduce its seal is refused.
/// [`tessera_core::Error::InexactInteger`] for an integer beyond ±(2^53−1).
pub fn manifest(m: &Manifest) -> Result<Vec<u8>> {
    canonical_form(&m.to_sealed_json()?)
}

/// The stored bytes of a sealed collection: the JCS form of [`Collection::to_sealed_json`].
///
/// Used by the `tessera collection` writer and the ingest engine's `collection.json`.
///
/// # Errors
/// As [`Collection::to_sealed_json`] and [`manifest`].
pub fn collection(c: &Collection) -> Result<Vec<u8>> {
    canonical_form(&c.to_sealed_json()?)
}

/// The stored form of sealed JSON text another writer produced: `JCS(parse_strict(json))`.
///
/// For the doors that copy a document instead of serialising a typed value (`pack_dir` re-packs an
/// exploded directory, `unpack` explodes a `.tsra`): the parsed object keeps every key it arrived
/// with, including newer-format fields this build doesn't model, so the seal over it is unchanged.
/// The caller verifies the seal first; this function only re-spells the bytes.
///
/// # Errors
/// As [`canonical::parse_strict`]: malformed JSON, a duplicate key, a lone surrogate, or an integer
/// beyond ±(2^53−1).
pub fn canonical_form(json: &str) -> Result<Vec<u8>> {
    canonical::to_bytes(&canonical::parse_strict(json)?)
}

// ── The ADR-0066 stored pair on disk (groundwork for the cutover, #914 part 2) ───────────────────
//
// Under ADR-0066 a sealed document is stored with its seal beside it: `manifest.json` +
// `manifest.seal` in the exploded form, `collection.json` + `collection.seal` beside a collection
// (rules 3 and 5). These helpers are the one place that pairing is spelled on disk. Until the
// cutover switches the exploded form and the collection files to it, nothing in this crate calls
// them; they are public API, and tested below.

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

/// Read a stored pair from disk: the document's bytes and its seal file's bytes (`None` when the
/// seal file does not exist, which [`Manifest::from_stored_members`] /
/// [`Collection::from_stored_members`] refuse as the alpha layout). Nothing is verified here.
///
/// # Errors
/// [`Error::Io`] when the document can't be read, or the seal file exists and can't be read;
/// [`Error::Invalid`] as [`seal_path`].
pub fn read_pair(document: &Path) -> Result<(Vec<u8>, Option<Vec<u8>>)> {
    let bytes = std::fs::read(document)?;
    let seal = match std::fs::read(seal_path(document)?) {
        Ok(s) => Some(s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.into()),
    };
    Ok((bytes, seal))
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn canonical_form_sorts_keys_drops_whitespace_and_uses_jcs_numbers() {
        let got = canonical_form("{ \"b\": 1.0, \"a\": [ 1e2, -0.0, \"\\u0041\" ] }").unwrap();
        assert_eq!(got, br#"{"a":[100,0,"A"],"b":1}"#);
    }

    #[test]
    fn canonical_form_is_idempotent() {
        let once = canonical_form("{\"z\": {\"y\": 0.1, \"x\": \"\\u00e9\"}}").unwrap();
        let twice = canonical_form(std::str::from_utf8(&once).unwrap()).unwrap();
        assert_eq!(once, twice);
    }

    #[test]
    fn canonical_form_keeps_i_json_at_the_door() {
        assert!(canonical_form(r#"{"a":1,"a":2}"#).is_err(), "duplicate key");
        assert!(
            canonical_form(r#"{"n":9007199254740993}"#).is_err(),
            "inexact integer"
        );
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
}
