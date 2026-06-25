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
