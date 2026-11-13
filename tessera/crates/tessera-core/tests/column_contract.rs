//! The column contract keyed by role (ADR-0065 §8, #809).

use serde_json::{json, Value};
use tessera_core::schema::{ReferenceScope, ReferenceTarget};
use tessera_core::{ProductSchema, SchemaRegistry};

/// The committed blake3 of every builtin schema's serialised bytes, captured before the column
/// contract existed.
const BUILTIN_DIGESTS: &str = include_str!("fixtures/builtin-schema-digests.txt");

/// Every builtin schema serialises to the bytes it had before `tables`/`arrays` existed, so every
/// schema already embedded in a sealed `.tsra` keeps its bytes and its seal.
#[test]
fn builtin_schemas_serialise_byte_identically() {
    let r = SchemaRegistry::builtin();
    let actual: String = r
        .products()
        .map(|p| {
            let bytes = serde_json::to_vec(r.get(p).unwrap()).unwrap();
            format!("{p} {}\n", blake3::hash(&bytes).to_hex())
        })
        .collect();
    assert_eq!(
        actual, BUILTIN_DIGESTS,
        "a builtin schema's serialised bytes moved"
    );
}

/// The #801 drafts, whose `*.schema.json` + `*.tables.json` pairs this contract was shaped by.
const DRAFTS: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/801-consumer-schemas"
);

/// Each draft's `*.schema.json` merged with its `*.tables.json` `tables` map parses as a
/// `ProductSchema`, and serialising it back keeps every draft key with its value: no key is dropped
/// or changed on the way through (the property #830's `deny_unknown_fields` will enforce at parse).
/// A key may be absent from the output only when its draft value is a default serde skips
/// (`"requires_generation": false`). So an *unknown* key holding `false`, `[]`, `{}` or `null`
/// would slip through here; #830's `deny_unknown_fields` closes that.
#[test]
fn the_801_drafts_parse_into_the_contract_without_losing_a_key() {
    let mut schemas: Vec<_> = std::fs::read_dir(DRAFTS)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.to_string_lossy().ends_with(".schema.json"))
        .collect();
    schemas.sort();
    let read = |p: &std::path::Path| -> Value {
        serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
    };
    let mut with_tables = 0;
    for path in &schemas {
        let mut merged = read(path);
        let tables_path = path
            .to_string_lossy()
            .replace(".schema.json", ".tables.json");
        if let Ok(text) = std::fs::read_to_string(&tables_path) {
            let tables: Value = serde_json::from_str(&text).unwrap();
            assert_eq!(
                tables["product"], merged["product"],
                "{tables_path}: product"
            );
            merged["tables"] = tables["tables"].clone();
            with_tables += 1;
        }
        let parsed = ProductSchema::from_value(&merged).unwrap();
        kept(
            &merged,
            &parsed.to_value().unwrap(),
            &path.display().to_string(),
        );
    }
    // Six products, five with a column contract (`geddf:pairwise-record` is metadata only): a
    // moved or emptied fixture directory must not pass vacuously.
    assert_eq!((schemas.len(), with_tables), (6, 5));
}

/// Assert `output` carries every key of `input` with an equal value, recursively; a key missing from
/// `output` must hold a default (`false`, `[]`, `{}` or `null`) in `input`.
fn kept(input: &Value, output: &Value, at: &str) {
    match (input, output) {
        (Value::Object(i), Value::Object(o)) => {
            for (k, v) in i {
                match o.get(k) {
                    Some(w) => kept(v, w, &format!("{at}.{k}")),
                    None => assert!(
                        matches!(v, Value::Bool(false) | Value::Null)
                            || v.as_array().is_some_and(Vec::is_empty)
                            || v.as_object().is_some_and(|m| m.is_empty()),
                        "{at}.{k} = {v} was dropped"
                    ),
                }
            }
        }
        (Value::Array(i), Value::Array(o)) => {
            assert_eq!(i.len(), o.len(), "{at}: length changed");
            for (n, (v, w)) in i.iter().zip(o).enumerate() {
                kept(v, w, &format!("{at}[{n}]"));
            }
        }
        _ => assert_eq!(input, output, "{at} changed"),
    }
}

/// A reference scope is `"block"` or `"source(<role>)"`; anything else is refused at parse rather
/// than carried as an unreadable string.
#[test]
fn reference_scopes_parse_strictly() {
    let target = |scope: &str| {
        serde_json::from_value::<ReferenceTarget>(
            json!({ "scope": scope, "block": "b", "columns": [] }),
        )
    };
    assert_eq!(target("block").unwrap().scope, ReferenceScope::Block);
    assert_eq!(
        target("source(crystal_map)").unwrap().scope,
        ReferenceScope::Source("crystal_map".into())
    );
    for bad in ["blocks", "source()", "source(x", "Source(x)"] {
        assert!(target(bad).is_err(), "{bad} parsed");
    }
}
