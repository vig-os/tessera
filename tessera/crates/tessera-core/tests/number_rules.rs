//! Schema-declared number rules (#895; ADR-0020 and ADR-0060 amendments of 2026-11-19, ADR-0066
//! rule 7): a 64-bit integer field declares a string encoding, and a numeric field may declare an
//! inclusive range. Both are checked at every seal, and by validation.

use serde_json::{json, Value};
use tessera_core::schema::{FieldDefinitionViolation, FieldSpec, FieldValueViolation};
use tessera_core::{validate_manifest, Error, ProductBuilder, ProductSchema, SchemaRegistry};

const TS: &str = "2024-01-01T00:00:00Z";

/// A one-field custom schema with field `n` declared as `n`.
fn schema_with(n: Value) -> Value {
    json!({
        "product": "counts",
        "version": "1.0",
        "description": "d",
        "fields": [n],
    })
}

/// Seal a `counts` product embedding `schema` with metadata `n = value`.
fn seal(schema: Value, value: Option<Value>) -> tessera_core::Result<tessera_core::Manifest> {
    let mut b = ProductBuilder::new("counts", "p", "d", TS);
    b.with_schema(schema);
    if let Some(v) = value {
        b.with_field("n", v);
    }
    b.seal()
}

#[test]
fn every_builtin_schema_follows_the_number_rules() {
    let r = SchemaRegistry::builtin();
    let products: Vec<&str> = r.products().collect();
    assert!(products.len() > 10, "the registry must not be empty here");
    for p in products {
        r.get(p).unwrap().check_definition().unwrap();
    }
}

/// A bare 64-bit field is refused, named by schema and field, before any value is looked at: at seal
/// (even with no value present) and at validate.
#[test]
fn a_schema_with_a_bare_64_bit_field_is_refused_by_name() {
    for dtype in ["uint64", "int64"] {
        let bare = schema_with(json!({ "id": "n", "description": "a count", "dtype": dtype }));
        let err = seal(bare.clone(), None).unwrap_err();
        match &err {
            Error::SchemaField {
                product,
                field,
                violation,
            } => {
                assert_eq!((product.as_str(), field.as_str()), ("counts", "n"));
                assert_eq!(
                    **violation,
                    FieldDefinitionViolation::ExceedsBound {
                        dtype: dtype.into()
                    }
                );
            }
            other => panic!("a bare {dtype} must be refused, got {other:?}"),
        }
        assert_eq!(
            err.to_string(),
            format!(
                "schema 'counts': field 'n' dtype {dtype} can hold integers beyond ±(2^53−1), the \
                 range a sealed JSON number holds exactly (SPEC §1, I-JSON RFC 7493 §2.2); declare \
                 \"encoding\": \"string\" to carry its values as decimal strings"
            )
        );
        // Validation refuses the same schema (here on a product a newer or foreign writer sealed).
        let mut m = ProductBuilder::new("counts", "p", "d", TS).seal().unwrap();
        m.schema = Some(bare);
        assert!(matches!(
            validate_manifest(&m),
            Err(Error::SchemaField { .. })
        ));
    }
}

/// `"encoding": "string"` is accepted, and its values are decimal strings by type.
#[test]
fn a_string_encoded_64_bit_field_carries_decimal_strings() {
    let encoded = schema_with(json!({
        "id": "n", "description": "a count", "dtype": "uint64", "encoding": "string"
    }));
    let m = seal(encoded.clone(), Some(json!("1730000000000000000"))).unwrap();
    assert_eq!(m.metadata["n"], json!("1730000000000000000"));
    validate_manifest(&m).unwrap();
    // The rule is the type's, not the value's magnitude: a small number is refused too.
    let err = seal(encoded, Some(json!(7))).unwrap_err();
    match &err {
        Error::FieldValue {
            origin,
            field,
            violation,
            ..
        } if matches!(**violation, FieldValueViolation::NotDecimalString { .. }) => {
            assert_eq!((origin.as_str(), field.as_str()), ("metadata.n", "n"));
        }
        other => panic!("a number in a string-encoded field must be refused, got {other:?}"),
    }
    assert!(err.to_string().starts_with(
        "metadata.n: field 'n' of schema 'counts': 7 is not a decimal string of uint64"
    ));
}

/// Out of range is refused, naming the field and where the value came from; in range seals exactly
/// as before the rule existed (the same bytes: the rule adds no key to the value).
#[test]
fn an_out_of_range_value_is_refused_and_an_in_range_value_is_unchanged() {
    let mut b = ProductBuilder::new("recon", "p", "d", TS);
    b.with_field("kvp", json!(120.5));
    let m = b.seal().unwrap();
    assert_eq!(m.metadata["kvp"], json!(120.5));
    let recon = SchemaRegistry::builtin().get("recon").unwrap().clone();
    recon.check_metadata(&m.metadata).unwrap();

    let mut b = ProductBuilder::new("recon", "p", "d", TS);
    b.with_field("kvp", json!(5000));
    let err = b.seal().unwrap_err();
    assert_eq!(
        err.to_string(),
        "metadata.kvp: field 'kvp' of schema 'recon': 5000 is above its maximum 1000"
    );

    // A door that knows the source names it instead (here as a CLI flag would).
    let err = recon
        .check_value("slice_thickness", &json!(-1.5), "--set slice_thickness")
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "--set slice_thickness: field 'slice_thickness' of schema 'recon': -1.5 is below its \
         minimum 0"
    );
    // An undeclared key is open-world, never range-checked.
    recon.check_value("not_a_field", &json!(-1), "x").unwrap();
}

/// A range on a schema a person writes follows the same rules (numeric, within the format bound).
#[test]
fn a_declared_range_must_be_within_the_format_bound() {
    let beyond = schema_with(json!({
        "id": "n", "description": "d", "dtype": "float64", "maximum": 1e300
    }));
    match seal(beyond, None) {
        Err(Error::SchemaField { violation, .. }) => assert!(matches!(
            *violation,
            FieldDefinitionViolation::BoundBeyondFormat {
                bound: "maximum",
                ..
            }
        )),
        other => panic!("a range beyond the bound must be refused, got {other:?}"),
    }
}

/// The new keys follow ADR-0060's newer-field rule: absent, they add no byte, so a schema that
/// declares none of them embeds exactly as before.
#[test]
fn the_new_keys_are_absent_when_unset() {
    let s = ProductSchema::from_value(&schema_with(
        json!({ "id": "n", "description": "d", "dtype": "float64" }),
    ))
    .unwrap();
    let v = s.to_value().unwrap();
    for key in ["encoding", "minimum", "maximum"] {
        assert!(v["fields"][0].get(key).is_none(), "{key}");
    }
    let f = FieldSpec::optional("n", "d", "uint64")
        .string_encoded()
        .range(0.0, 10.0);
    let v = serde_json::to_value(&f).unwrap();
    assert_eq!(
        (&v["encoding"], &v["minimum"], &v["maximum"]),
        (&json!("string"), &json!(0.0), &json!(10.0))
    );
}
