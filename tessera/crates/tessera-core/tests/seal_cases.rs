//! The Rust reader reaches every verdict in `corpus/seal-cases.json`, the file the independent
//! reference reader (`corpus/reference_reader/run.py`, the `reference-reader` flake check) checks
//! too. Both readers held to one file of verdicts agree by construction (#573, SPEC §4).
//!
//! The cases are the 8 corpus manifests as sealed, each with a newer-format field added and resealed
//! (must verify), and each with a field spliced in under the old seal (must fail), plus the encodings
//! a struct round-trip loses (`"study": null`, an omitted `sources`, a nested newer field), a splice
//! whose seal was deleted or nulled (tampered: a missing seal is not a pass), and text that is not
//! I-JSON (duplicate keys, NaN, a lone surrogate: refused).

use serde_json::Value;
use tessera_core::{Error, Manifest};

fn cases() -> Vec<Value> {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../corpus/seal-cases.json");
    let text = std::fs::read_to_string(path).expect("corpus/seal-cases.json present");
    let doc: Value = serde_json::from_str(&text).expect("seal-cases.json parses");
    doc["cases"].as_array().expect("cases array").clone()
}

/// The verdict the Rust reader reaches, in the file's vocabulary, plus the view's newer fields.
fn rust_verdict(manifest: &str) -> (&'static str, Vec<String>) {
    match Manifest::from_json_verified(manifest) {
        Ok(m) => ("ok", m.newer_fields().to_vec()),
        Err(Error::Integrity { .. }) => ("tampered", Vec::new()),
        // Not I-JSON: a duplicate key, NaN, a lone surrogate (all refused by the parser).
        Err(Error::Serde(_)) => ("refused", Vec::new()),
        Err(Error::UnsupportedVersion(_)) => ("refused", Vec::new()),
        // A block name listed twice (SPEC §5, #734), refused before any hash.
        Err(Error::DuplicateBlockName { .. }) => ("refused", Vec::new()),
        Err(e) => panic!("unexpected error class: {e}"),
    }
}

#[test]
fn rust_reader_reaches_every_seal_case_verdict() {
    let cases = cases();
    assert_eq!(cases.len(), 41, "seal-cases.json lost or gained cases");
    let mut disagreements = Vec::new();
    for c in &cases {
        let name = c["name"].as_str().unwrap();
        let (verdict, newer) = rust_verdict(c["manifest"].as_str().unwrap());
        let want_newer: Vec<String> =
            serde_json::from_value(c["newer_fields"].clone()).expect("newer_fields list");
        if verdict != c["verdict"].as_str().unwrap() || newer != want_newer {
            disagreements.push(format!(
                "{name}: expected {} {want_newer:?}, got {verdict} {newer:?}",
                c["verdict"]
            ));
        }
    }
    assert!(disagreements.is_empty(), "{disagreements:#?}");
}

/// Every verdict class is present for each corpus manifest, so the gate can't pass on a file that
/// lost its B (newer field, resealed) or C (spliced field) cases.
#[test]
fn every_corpus_manifest_has_its_b_and_c_case() {
    let cases = cases();
    let names: Vec<&str> = cases.iter().map(|c| c["name"].as_str().unwrap()).collect();
    let corpus = names.iter().filter_map(|n| n.strip_suffix("/as-sealed"));
    let mut count = 0;
    for base in corpus {
        count += 1;
        for suffix in ["newer-field-resealed", "field-spliced"] {
            let want = format!("{base}/{suffix}");
            assert!(names.contains(&want.as_str()), "missing case {want}");
        }
    }
    assert_eq!(count, 8, "one as-sealed case per corpus file");
}

fn case(name: &str) -> String {
    cases()
        .into_iter()
        .find(|c| c["name"] == name)
        .unwrap_or_else(|| panic!("no case {name}"))["manifest"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// A view with newer fields can be read and verified, but nothing re-seals or re-emits it: the
/// struct doesn't carry the fields, so either would drop sealed content (ADR-0060 §2.2).
#[test]
fn a_view_with_newer_fields_is_never_resealed_or_reemitted() {
    let m = Manifest::from_json_verified(&case("recon_int16/newer-field-resealed")).unwrap();
    assert_eq!(m.newer_fields(), ["actors"]);
    let refused = |r: tessera_core::Result<()>| {
        assert!(
            matches!(r, Err(Error::NewerFormatFields { ref fields }) if fields == &["actors"]),
            "{r:?}"
        )
    };
    refused(tessera_core::ProductBuilder::from_manifest(&m).map(drop));
    refused(m.compute_manifest_hash().map(drop));
    refused(m.to_sealed_json().map(drop));
    // The struct check names the real reason instead of crying tampering.
    refused(m.verify());
}

/// A view of an encoding the struct normalises still verifies as bytes, but is not re-emitted as
/// its own JSON, which would no longer carry the seal.
#[test]
fn a_normalised_view_is_not_reemitted_under_its_seal() {
    for name in [
        "recon_int16/study-null-resealed",
        "recon_int16/sources-omitted-resealed",
    ] {
        let m = Manifest::from_json_verified(&case(name)).unwrap();
        assert!(m.newer_fields().is_empty(), "{name}");
        let err = m.to_sealed_json().unwrap_err();
        assert!(matches!(err, Error::Invalid(_)), "{name}: {err}");
    }
}

/// The seal a writer stamps is the reader's function over the writer's own output, for every
/// corpus manifest: writer and reader can't drift.
#[test]
fn writer_seal_is_the_reader_seal_over_the_written_json() {
    for c in cases() {
        let name = c["name"].as_str().unwrap();
        if !name.ends_with("/as-sealed") {
            continue;
        }
        let m = Manifest::from_json_verified(c["manifest"].as_str().unwrap()).unwrap();
        let written = m.to_sealed_json().unwrap();
        let object = tessera_core::canonical::parse_strict(&written).unwrap();
        assert_eq!(
            tessera_core::manifest::seal_of(&object).unwrap(),
            m.compute_manifest_hash().unwrap(),
            "{name}"
        );
        assert_eq!(
            Some(m.compute_manifest_hash().unwrap()),
            m.manifest_hash,
            "{name}: golden seal unchanged"
        );
    }
}

#[test]
fn parse_strict_refuses_trailing_input_and_non_objects_cannot_be_sealed() {
    assert!(tessera_core::canonical::parse_strict(r#"{"a":1} {"a":2}"#).is_err());
    let not_object = serde_json::json!([1, 2]);
    assert!(matches!(
        tessera_core::manifest::seal_of(&not_object),
        Err(Error::Canonicalization(_))
    ));
}

/// The duplicate-key cases are refused for being duplicates, at the top level, nested, and after
/// unescaping: not by some other parse error that would hide a regression in the check.
#[test]
fn duplicate_keys_are_refused_by_name() {
    for name in [
        "recon_int16/duplicate-top-level-key",
        "recon_int16/duplicate-key-after-unescaping",
        "recon_int16/duplicate-nested-key",
    ] {
        let err = Manifest::from_json_verified(&case(name)).unwrap_err();
        assert!(
            matches!(&err, Error::Serde(e) if e.to_string().contains("duplicate key")),
            "{name}: {err}"
        );
    }
}
