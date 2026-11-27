//! Collection writer rules apply after public-field mutations too (ADR-0067 D2/D5, #997).

use tessera_core::provenance::{Source, FORKED_FROM_ROLE, SUPERSEDES_ROLE};
use tessera_core::{Collection, CollectionBuilder, Error};

const TS: &str = "2024-01-01T00:00:00Z";

fn invalid<T: std::fmt::Debug>(result: tessera_core::Result<T>) {
    assert!(matches!(result, Err(Error::Invalid(_))), "{result:?}");
}

#[test]
fn probe_to_stored_bypasses_fork_rule() {
    let mut c = CollectionBuilder::new("c", "d", TS).seal().unwrap();
    c.sources.push(
        Source::new(FORKED_FROM_ROLE, &c.id).with_content_hash(c.manifest_hash.as_ref().unwrap()),
    );
    // Forge a matching seal directly, without invoking a writer door before to_stored.
    let bytes = tessera_core::canonical::to_bytes(&serde_json::to_value(&c).unwrap()).unwrap();
    c.manifest_hash = Some(tessera_core::hash::digest(&bytes));
    invalid(c.to_stored());
}

fn source_collection(role: &str, token: Option<&str>, pin: Option<&str>) -> Collection {
    let mut c = Collection::new("c", "d", TS);
    if let Some(token) = token {
        c.id_inputs.insert("fork".into(), token.into());
        c.id = c.recompute_id().unwrap();
    }
    c.content_hash = Some(c.recompute_content_hash());
    let mut source = Source::new(role, "predecessor");
    source.content_hash = pin.map(str::to_owned);
    c.sources.push(source);
    c
}

fn all_writer_doors_refuse(mut c: Collection) {
    // Construct honestly sealed input without going through a writer API, as another reader
    // would supply it. Reader acceptance does not authorise this build to write a new seal.
    let bytes = tessera_core::canonical::to_bytes(&serde_json::to_value(&c).unwrap()).unwrap();
    let hash = tessera_core::hash::digest(&bytes);
    c.manifest_hash = Some(hash.clone());
    let read = Collection::from_stored(&bytes, &hash).unwrap();
    assert!(
        read.to_json().is_ok(),
        "display remains available to readers"
    );
    invalid(c.canonical_bytes());
    invalid(c.compute_manifest_hash());
    invalid(c.to_stored());
    invalid(read.to_stored());
    let mut builder = CollectionBuilder::new("c", "d", TS);
    builder
        .with_id_inputs(c.id_inputs.clone())
        .add_source(c.sources[0].clone());
    invalid(builder.seal());
}

#[test]
fn writer_doors_require_a_fork_token_after_mutation() {
    let pin = tessera_core::hash::digest(b"predecessor");
    all_writer_doors_refuse(source_collection(FORKED_FROM_ROLE, None, Some(&pin)));
}

#[test]
fn writer_doors_refuse_an_empty_fork_token() {
    let pin = tessera_core::hash::digest(b"predecessor");
    all_writer_doors_refuse(source_collection(FORKED_FROM_ROLE, Some(""), Some(&pin)));
}

#[test]
fn writer_doors_refuse_a_whitespace_only_fork_token() {
    let pin = tessera_core::hash::digest(b"predecessor");
    for token in [" ", "\t\r\n", "\u{2003}"] {
        let collection = source_collection(FORKED_FROM_ROLE, Some(token), Some(&pin));
        let error = collection.canonical_bytes().unwrap_err();
        assert!(error.to_string().contains("whitespace-only"), "{error}");
        all_writer_doors_refuse(collection);
    }
}

#[test]
fn supersedes_requires_a_version_pin() {
    for pin in [None, Some("")] {
        all_writer_doors_refuse(source_collection(SUPERSEDES_ROLE, None, pin));
    }
}

#[test]
fn forked_from_requires_a_version_pin() {
    for pin in [None, Some("")] {
        all_writer_doors_refuse(source_collection(FORKED_FROM_ROLE, Some("node:1"), pin));
    }
}

#[test]
fn other_roles_can_still_be_reference_only() {
    for role in ["derived_from", "merged_from", "external"] {
        let c = source_collection(role, None, None);
        let bytes = c.canonical_bytes().unwrap();
        let (stored, _) = c.to_stored().unwrap();
        assert_eq!(stored, bytes);
    }
}
