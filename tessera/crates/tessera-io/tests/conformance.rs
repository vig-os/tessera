//! Conformance: the deterministic fixtures must reproduce the golden hashes pinned in
//! `tessera/corpus/corpus.json`, and every fixture must pack to a byte-identical `.tsra` and
//! verify on open. A drift here means either a deliberate format change (regenerate the corpus +
//! bump the spec version via `cargo run -p tessera-io --example gen_corpus`) or a bug.

use std::collections::BTreeMap;

use tessera_io::conformance::{fixtures, Golden};

fn golden_index() -> BTreeMap<String, Golden> {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../corpus/corpus.json");
    let json = std::fs::read_to_string(path).expect("corpus/corpus.json present");
    serde_json::from_str::<Vec<Golden>>(&json)
        .expect("corpus.json parses")
        .into_iter()
        .map(|g| (g.name.clone(), g))
        .collect()
}

#[test]
fn corpus_hashes_match_goldens() {
    let golden = golden_index();
    for f in fixtures() {
        let g = golden
            .get(f.name)
            .unwrap_or_else(|| panic!("no golden record for fixture '{}'", f.name));
        assert_eq!(
            &f.golden(),
            g,
            "fixture '{}' drifted from its golden id/content_hash/manifest_hash",
            f.name
        );
    }
    assert_eq!(
        fixtures().len(),
        golden.len(),
        "fixture count != golden count (regenerate corpus.json)"
    );
}

#[test]
fn corpus_packs_deterministically_and_verifies() {
    let dir = tempfile::tempdir().unwrap();
    for f in fixtures() {
        let p1 = dir.path().join(format!("{}-1.tsra", f.name));
        let p2 = dir.path().join(format!("{}-2.tsra", f.name));
        tessera_io::pack(&f.manifest, &f.payloads, &p1).unwrap();
        tessera_io::pack(&f.manifest, &f.payloads, &p2).unwrap();
        assert_eq!(
            std::fs::read(&p1).unwrap(),
            std::fs::read(&p2).unwrap(),
            "fixture '{}' did not pack byte-identically (writer-determinism gate)",
            f.name
        );
        let mut r = tessera_io::Reader::open(&p1).unwrap();
        for n in r.block_names() {
            r.read_block(&n).unwrap();
        }
    }
}

/// A fresh `pack` of every fixture is **byte-identical** to its committed `corpus/files/<name>.tsra`.
///
/// The golden triple pins the manifest, and `corpus_packs_deterministically_and_verifies` pins
/// same-build determinism, but neither sees the container framing: the local headers, the central
/// directory and the zip64 records that the `zip` crate writes around the sealed bytes. A `zip`
/// upgrade or an entry-option change that reframes the archive keeps every hash and still verifies,
/// yet changes the file an independent reader consumes. This is the gate that catches it (#588, the
/// zip 2 → 9 upgrade, passed it unchanged). The other writers are pinned to `pack` byte-for-byte by
/// the `pack_streaming_*` tests in `container.rs`, so they inherit it.
///
/// If this fails after an intended format change, regenerate with
/// `cargo run -p tessera-io --example gen_corpus_files` and treat it as a corpus event (ADR-0022).
#[test]
fn committed_corpus_files_are_byte_identical_to_a_fresh_pack() {
    let dir = tempfile::tempdir().unwrap();
    let committed = concat!(env!("CARGO_MANIFEST_DIR"), "/../../corpus/files");
    let all = fixtures();
    assert!(
        !all.is_empty(),
        "no conformance fixtures: the loop below would prove nothing"
    );
    for f in all {
        let fresh = dir.path().join(format!("{}.tsra", f.name));
        tessera_io::pack(&f.manifest, &f.payloads, &fresh).unwrap();
        let want_path = std::path::Path::new(committed).join(format!("{}.tsra", f.name));
        let want = std::fs::read(&want_path)
            .unwrap_or_else(|e| panic!("read committed corpus file '{}': {e}", f.name));
        let got = std::fs::read(&fresh).unwrap();
        assert!(
            got == want,
            "fixture '{}': a fresh pack ({} bytes) differs from the committed {} ({} bytes), first \
             difference at byte {:?} — the container framing moved",
            f.name,
            got.len(),
            want_path.display(),
            want.len(),
            got.iter().zip(&want).position(|(a, b)| a != b)
        );
    }
}

/// The committed `corpus/files/<name>.tsra` test vectors (what an independent v1.0 reader consumes)
/// must reproduce the golden triple and verify on open. Regenerate after an intended format change:
/// `cargo run -p tessera-io --example gen_corpus_files`.
#[test]
fn committed_corpus_files_reproduce_goldens() {
    let golden = golden_index();
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../corpus/files");
    for f in fixtures() {
        let path = std::path::Path::new(dir).join(format!("{}.tsra", f.name));
        let mut r = tessera_io::Reader::open(&path)
            .unwrap_or_else(|e| panic!("open committed corpus file '{}': {e}", f.name));
        let m = r.manifest().clone();
        let g = golden
            .get(f.name)
            .unwrap_or_else(|| panic!("no golden for '{}'", f.name));
        assert_eq!(m.id, g.id, "{} id", f.name);
        assert_eq!(
            m.content_hash.as_deref().unwrap_or_default(),
            g.content_hash,
            "{} content_hash",
            f.name
        );
        assert_eq!(
            m.manifest_hash.as_deref().unwrap_or_default(),
            g.manifest_hash,
            "{} manifest_hash",
            f.name
        );
        for n in r.block_names() {
            r.read_block(&n).unwrap(); // each stored block verifies against its digest
        }
    }
}
