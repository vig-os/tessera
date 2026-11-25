//! The Rust reader's verdict on every case of `corpus/schema-cases.json` (#895): a product schema's
//! number rules, which the reference reader (`corpus/reference_reader`) checks over the same file, so
//! the two readers are compared case by case.

use serde_json::Value;
use tessera_core::{Error, ProductSchema};

const CASES: &str = include_str!("../../../corpus/schema-cases.json");

/// `ok`, `schema` (a field declaration is refused) or `value` (a metadata value is refused).
fn verdict(schema: &Value, metadata: &Value) -> String {
    let schema = ProductSchema::from_value(schema).expect("every case's schema parses");
    let metadata = serde_json::from_value(metadata.clone()).expect("metadata is an object");
    match schema
        .check_definition()
        .and_then(|()| schema.check_metadata(&metadata))
    {
        Ok(()) => "ok".into(),
        Err(Error::SchemaField { .. }) => "schema".into(),
        Err(Error::FieldValue { .. }) => "value".into(),
        Err(e) => panic!("not a number-rule verdict: {e}"),
    }
}

#[test]
fn rust_reader_reaches_every_schema_case_verdict() {
    let doc: Value = serde_json::from_str(CASES).unwrap();
    let cases = doc["cases"].as_array().unwrap();
    assert!(cases.len() >= 40, "the case file must not be empty here");
    let mut seen = std::collections::BTreeSet::new();
    let disagreements: Vec<String> = cases
        .iter()
        .filter_map(|c| {
            let name = c["name"].as_str().unwrap();
            assert!(seen.insert(name.to_owned()), "case {name} listed twice");
            let got = verdict(&c["schema"], &c["metadata"]);
            (got != c["verdict"]).then(|| format!("{name}: expected {}, got {got}", c["verdict"]))
        })
        .collect();
    assert!(disagreements.is_empty(), "{disagreements:#?}");
    // Each verdict occurs, so a reader answering one verdict for everything fails here.
    for v in ["ok", "schema", "value"] {
        assert!(cases.iter().any(|c| c["verdict"] == v), "no {v} case");
    }
}
