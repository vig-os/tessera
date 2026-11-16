//! The column contract keyed by role (ADR-0065 §8, #809).

use serde_json::{json, Value};
use tessera_core::block::array::ArraySpec;
use tessera_core::block::table::{Column, TableSpec};
use tessera_core::block::BlockKind;
use tessera_core::schema::{
    ArrayContract, ContractViolation, ReferenceScope, ReferenceTarget, TableContract,
};
use tessera_core::{Error, ProductSchema, SchemaRegistry};

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

/// A product schema with one table role and one array role, for the refusal tests.
fn schema() -> ProductSchema {
    let mut s = ProductSchema::from_value(&json!({
        "product": "test:contract", "version": "1", "description": "",
    }))
    .unwrap();
    s.tables.insert(
        "events".into(),
        TableContract {
            columns: vec![
                Column::new("event_id", "u8"),
                Column::new("n_hits", "u4").optional(),
                Column::new("energy", "f4").nullable(),
            ],
            sequence_key: vec!["event_id".into()],
            ..Default::default()
        },
    );
    s.arrays.insert(
        "lyso_map".into(),
        ArrayContract {
            dtype: "float32".into(),
            rank: 2,
            axes: vec!["y".into(), "x".into()],
            unit: None,
        },
    );
    s
}

fn table(columns: &[Column]) -> TableSpec {
    TableSpec {
        columns: columns.to_vec(),
        rows: 0,
        row_index: None,
        partition: None,
        edition: None,
    }
}

fn violation(r: tessera_core::Result<()>) -> ContractViolation {
    match r {
        Err(Error::Contract { product, violation }) => {
            assert_eq!(product, "test:contract");
            violation
        }
        other => panic!("expected a contract violation, got {other:?}"),
    }
}

fn id() -> Column {
    Column::new("event_id", "u8")
}
fn hits() -> Column {
    Column::new("n_hits", "u4")
}
fn energy() -> Column {
    Column::new("energy", "f4").nullable()
}

#[test]
fn a_conforming_table_passes_with_or_without_its_optional_column() {
    let s = schema();
    s.check_table("events", &table(&[id(), hits(), energy()]))
        .unwrap();
    s.check_table("events", &table(&[id(), energy()])).unwrap();
}

#[test]
fn an_undeclared_role_is_refused() {
    let s = schema();
    let v = violation(s.check_table("pethits", &table(&[id()])));
    assert!(
        matches!(v, ContractViolation::UnknownRole { kind: BlockKind::Table, ref role } if role == "pethits")
    );
    // A role declared for the other kind is not a table role.
    let v = violation(s.check_table("lyso_map", &table(&[id()])));
    assert!(matches!(
        v,
        ContractViolation::UnknownRole {
            kind: BlockKind::Table,
            ..
        }
    ));
    let v = violation(s.check_array("events", &array("float32", &[4, 4], &["y", "x"])));
    assert!(matches!(
        v,
        ContractViolation::UnknownRole {
            kind: BlockKind::Array,
            ..
        }
    ));
}

#[test]
fn an_undeclared_or_repeated_column_is_refused() {
    let s = schema();
    let v = violation(s.check_table(
        "events",
        &table(&[id(), Column::new("extra", "u1"), energy()]),
    ));
    assert!(matches!(v, ContractViolation::UnknownColumn { ref column, .. } if column == "extra"));
    let v = violation(s.check_table("events", &table(&[id(), id(), energy()])));
    assert!(
        matches!(v, ContractViolation::DuplicateColumn { ref column, .. } if column == "event_id")
    );
}

#[test]
fn columns_out_of_the_declared_order_are_refused() {
    let s = schema();
    let v = violation(s.check_table("events", &table(&[energy(), id()])));
    assert_eq!(
        v,
        ContractViolation::ColumnOrder {
            role: "events".into(),
            column: "event_id".into(),
            block_position: 1
        }
    );
    // An optional column is still bound to its place when present.
    let v = violation(s.check_table("events", &table(&[id(), energy(), hits()])));
    assert!(matches!(v, ContractViolation::ColumnOrder { ref column, .. } if column == "n_hits"));
}

#[test]
fn a_missing_non_optional_column_is_refused() {
    let s = schema();
    let v = violation(s.check_table("events", &table(&[id(), hits()])));
    assert_eq!(
        v,
        ContractViolation::MissingColumn {
            role: "events".into(),
            column: "energy".into()
        }
    );
}

#[test]
fn a_column_dtype_mismatch_is_refused() {
    let s = schema();
    let v = violation(s.check_table("events", &table(&[Column::new("event_id", "i8"), energy()])));
    assert_eq!(
        v,
        ContractViolation::ColumnDtype {
            role: "events".into(),
            column: "event_id".into(),
            declared: "u8".into(),
            found: "i8".into(),
        }
    );
}

#[test]
fn a_nullability_mismatch_is_refused_in_both_directions() {
    let s = schema();
    let v = violation(s.check_table("events", &table(&[id(), Column::new("energy", "f4")])));
    assert!(matches!(
        v,
        ContractViolation::ColumnNullability {
            declared: true,
            found: false,
            ..
        }
    ));
    let v = violation(s.check_table("events", &table(&[id().nullable(), energy()])));
    assert!(matches!(
        v,
        ContractViolation::ColumnNullability {
            declared: false,
            found: true,
            ..
        }
    ));
}

fn array(dtype: &str, shape: &[u64], axes: &[&str]) -> ArraySpec {
    ArraySpec {
        axes: axes.iter().map(|a| a.to_string()).collect(),
        ..ArraySpec::new(shape.to_vec(), dtype)
    }
}

#[test]
fn array_dtype_rank_and_axes_mismatches_are_refused() {
    let s = schema();
    s.check_array("lyso_map", &array("float32", &[64, 64], &["y", "x"]))
        .unwrap();
    let v = violation(s.check_array("lyso_map", &array("float64", &[64, 64], &["y", "x"])));
    assert!(matches!(v, ContractViolation::ArrayDtype { ref found, .. } if found == "float64"));
    let v = violation(s.check_array(
        "lyso_map",
        &array("float32", &[4, 64, 64], &["z", "y", "x"]),
    ));
    assert!(matches!(
        v,
        ContractViolation::ArrayRank {
            declared: 2,
            found: 3,
            ..
        }
    ));
    let v = violation(s.check_array("lyso_map", &array("float32", &[64, 64], &["x", "y"])));
    assert!(matches!(v, ContractViolation::ArrayAxes { .. }));
}

/// An array contract without axes leaves them to the producer: any axis names pass.
#[test]
fn an_array_contract_without_axes_accepts_any_axes() {
    let mut s = schema();
    s.arrays.get_mut("lyso_map").unwrap().axes.clear();
    s.check_array("lyso_map", &array("float32", &[64, 64], &["x", "y"]))
        .unwrap();
    s.check_array("lyso_map", &array("float32", &[64, 64], &[]))
        .unwrap();
}

/// A contract listing a column twice parses (its own consistency is #844) but no block satisfies
/// it: the repeated column is refused as out of order, never passed.
#[test]
fn a_contract_with_a_repeated_column_refuses_every_block() {
    let mut s = schema();
    s.tables.get_mut("events").unwrap().columns = vec![id(), id()];
    for block in [table(&[id()]), table(&[id(), id()])] {
        let v = violation(s.check_table("events", &block));
        assert!(matches!(
            v,
            ContractViolation::ColumnOrder { .. } | ContractViolation::DuplicateColumn { .. }
        ));
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

/// The serialisation rule, pinned byte for byte before any contract is sealed (moving it later
/// moves sealed bytes): `columns`, an array's `dtype` and `rank` are always written; every other
/// contract field is left out at its default, so an empty `sequence_key`, `keys` or `references`
/// serialises exactly as an absent one. A full contract writes every field.
#[test]
fn contract_fields_serialise_skip_if_default_byte_for_byte() {
    let bare = TableContract {
        columns: vec![Column::new("event_id", "u8")],
        ..Default::default()
    };
    assert_eq!(
        serde_json::to_string(&bare).unwrap(),
        r#"{"columns":[{"name":"event_id","dtype":"u8"}]}"#
    );
    let parsed: TableContract =
        serde_json::from_str(r#"{"columns":[{"name":"event_id","dtype":"u8"}],"sequence_key":[],"keys":[],"references":[]}"#)
            .unwrap();
    assert_eq!(parsed, bare, "empty and absent parse the same");

    let full: TableContract = serde_json::from_value(json!({
        "columns": [{ "name": "event_id", "dtype": "u8", "optional": true }],
        "sequence_key": ["event_id"],
        "order": "producer",
        "keys": [["event_id"]],
        "references": [{
            "columns": ["event_id"],
            "to": { "scope": "source(sim)", "block": "events", "columns": ["event_id"] },
            "note": "n",
        }],
    }))
    .unwrap();
    assert_eq!(
        serde_json::to_string(&full).unwrap(),
        concat!(
            r#"{"columns":[{"name":"event_id","dtype":"u8","optional":true}],"sequence_key":["event_id"],"#,
            r#""order":"producer","keys":[["event_id"]],"references":[{"columns":["event_id"],"#,
            r#""to":{"scope":"source(sim)","block":"events","columns":["event_id"]},"note":"n"}]}"#,
        )
    );

    let array = ArrayContract {
        dtype: "int16".into(),
        rank: 3,
        axes: vec![],
        unit: None,
    };
    assert_eq!(
        serde_json::to_string(&array).unwrap(),
        r#"{"dtype":"int16","rank":3}"#
    );
}
