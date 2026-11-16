//! Forward compatibility of collections and embedded product schemas (#873, ADR-0060 §2.2 / §11 P0b).
//!
//! The #573 rule, applied beyond the product manifest: a collection's seal is verified over the JSON
//! object as parsed (SPEC §4), so a key a newer writer added and sealed verifies and is named, a key
//! spliced in under an old seal is tampering, and a repeated key is refused. An embedded schema that
//! carries a rule this build doesn't model is refused at validate rather than reported valid.

use serde_json::{json, Value};
use tessera_core::canonical::hash_without;
use tessera_core::collection::{CollectionBuilder, CollectionHandle, ProductHandle};
use tessera_core::schema::Coded;
use tessera_core::{validate_manifest, Collection, Error, Manifest, ProductBuilder, Role};

const TS: &str = "2024-01-01T00:00:00Z";

/// A sealed collection with a product member, a sub-collection member and a `level` tag, as the
/// JSON object a writer emits.
fn sealed_collection() -> Value {
    let p = ProductBuilder::new("recon", "p", "d", TS).seal().unwrap();
    let mut child = CollectionBuilder::new("child", "d", TS);
    child.add_product(&ProductHandle::of(&p).unwrap(), Role::Raw, Vec::new());
    let child = child.seal().unwrap();
    let mut b = CollectionBuilder::new("cohort", "a cohort", TS);
    b.with_level(Coded::new("tessera-imaging-levels", "cohort"));
    b.add_product(&ProductHandle::of(&p).unwrap(), Role::Raw, Vec::new());
    b.add_subcollection(
        &CollectionHandle::of(&child).unwrap(),
        Role::Derived,
        vec![p.id.clone()],
    );
    serde_json::from_str(&b.seal().unwrap().to_json().unwrap()).unwrap()
}

/// Seal `v` the way SPEC §4 defines it: over the object with only `manifest_hash` removed.
fn reseal(v: &mut Value) {
    v["manifest_hash"] = json!(hash_without(v, "manifest_hash").unwrap());
}

fn verify(v: &Value) -> tessera_core::Result<Collection> {
    Collection::from_json_verified(&v.to_string())
}

/// Set the key at the end of `path` (object keys, or array indices as digits) to `value`.
fn set(v: &mut Value, path: &[&str], value: Value) {
    let (last, parents) = path.split_last().unwrap();
    let mut at = v;
    for seg in parents {
        at = match seg.parse::<usize>() {
            Ok(i) => &mut at[i],
            Err(_) => &mut at[*seg],
        };
    }
    at[*last] = value;
}

fn integrity(r: tessera_core::Result<Collection>) -> &'static str {
    match r {
        Err(Error::Integrity { what, .. }) => what,
        other => panic!("expected an integrity failure, got {other:?}"),
    }
}

#[test]
fn collection_forward_compat() {
    let sealed = sealed_collection();
    let c = verify(&sealed).expect("the writer's own collection verifies");
    assert!(c.newer_fields().is_empty());

    // A newer writer added fields at the top level, inside `level` and inside a member, and sealed
    // them: the collection verifies, and every field the view doesn't model is named by path.
    let mut newer = sealed.clone();
    newer["annotations"] = json!({ "curator": "site-a" });
    newer["level"]["uri"] = json!("https://example.org/levels#cohort");
    newer["members"][1]["weight"] = json!(0.5);
    reseal(&mut newer);
    let c = verify(&newer).expect("a sealed newer field verifies");
    assert_eq!(
        c.newer_fields(),
        ["annotations", "level.uri", "members[1].weight"]
    );
    // ... and nothing builds a new seal from a view that would drop them.
    for refused in [
        c.compute_manifest_hash().map(drop),
        c.canonical_bytes().map(drop),
    ] {
        match refused {
            Err(Error::NewerFormatFields { fields }) => assert_eq!(fields, c.newer_fields()),
            other => panic!("a re-seal must refuse the newer fields, got {other:?}"),
        }
    }
    assert!(matches!(c.verify(), Err(Error::NewerFormatFields { .. })));

    // An encoding the struct re-emits differently still verifies as sealed: an older writer's
    // collection without the `collection_schema` key (the struct fills in `"collection"`).
    let mut older = sealed.clone();
    older.as_object_mut().unwrap().remove("collection_schema");
    reseal(&mut older);
    let c = verify(&older).expect("an omitted defaulted key verifies as sealed");
    assert!(c.newer_fields().is_empty());

    // The same field spliced in under the old seal is tampering, at any depth.
    for path in [
        &["forged"][..],
        &["level", "forged"],
        &["members", "0", "forged"],
    ] {
        let mut spliced = sealed.clone();
        set(&mut spliced, path, json!(true));
        assert_eq!(integrity(verify(&spliced)), "manifest_hash", "{path:?}");
    }

    // A deleted or nulled seal doesn't launder a splice: an unsealed collection doesn't verify.
    for drop_key in ["manifest_hash", "content_hash"] {
        let mut unsealed = sealed.clone();
        unsealed["forged"] = json!(true);
        unsealed.as_object_mut().unwrap().remove(drop_key);
        assert_eq!(integrity(verify(&unsealed)), drop_key);
    }
}

#[test]
fn collection_duplicate_keys_are_refused() {
    let text = sealed_collection().to_string();
    // A repeated key anywhere (top level, inside an opaque map, inside a member) reads differently
    // under first-wins and last-wins parsers, so the same bytes can't carry one seal.
    let cases = [
        text.replacen("{", r#"{"name":"forged","#, 1),
        text.replacen(r#""id_inputs":{"#, r#""id_inputs":{"name":"forged","#, 1),
        text.replacen(r#""role":"raw""#, r#""role":"derived","role":"raw""#, 1),
    ];
    for dup in cases {
        assert_ne!(dup, text, "the fixture must contain the spliced anchor");
        match Collection::from_json_verified(&dup) {
            Err(Error::Serde(e)) => assert!(e.to_string().contains("duplicate key"), "{e}"),
            other => panic!("a duplicate key must be refused, got {other:?}"),
        }
    }
}

/// A product whose embedded schema declares one optional field and one optional block role.
fn with_schema(schema: Value) -> Manifest {
    let mut b = ProductBuilder::new("custom", "p", "d", TS);
    b.with_schema(schema);
    b.seal().unwrap()
}

fn base_schema() -> Value {
    json!({
        "product": "custom",
        "version": "1.0",
        "description": "d",
        "fields": [{ "id": "site", "description": "acquisition site", "dtype": "string" }],
        "blocks": [{ "role": "volume", "min_count": 0, "description": "the volume" }],
        "tables": { "events": { "columns": [{ "name": "t", "dtype": "u8" }] } },
        "arrays": { "lyso_map": { "dtype": "float32", "rank": 2 } }
    })
}

#[test]
fn unknown_schema_rule_is_not_validated() {
    validate_manifest(&with_schema(base_schema())).expect("the known schema validates");

    // Known keys holding their defaults are not newer fields, even though the struct drops them
    // when it writes the schema back out.
    let mut defaults = base_schema();
    defaults["requires_generation"] = json!(false);
    defaults["fields"][0]["inherit"] = json!(false);
    defaults["fields"][0]["unit"] = Value::Null;
    validate_manifest(&with_schema(defaults)).expect("explicit defaults validate");

    // `default` is an opaque value (data, not format): its keys are never newer fields.
    let mut opaque = base_schema();
    opaque["fields"][0]["default"] = json!({ "nested": 1 });
    validate_manifest(&with_schema(opaque)).expect("an opaque default is not walked");

    // A rule this build doesn't model, at any depth, is refused: reporting "valid" against a rule
    // the validator never evaluated would be a lie.
    let cases: [&[&str]; 6] = [
        &["max_blocks"],
        &["fields", "0", "pattern"],
        &["blocks", "0", "max_count"],
        &["tables", "events", "row_count_min"],
        &["tables", "events", "columns", "0", "monotonic"],
        &["arrays", "lyso_map", "range"],
    ];
    let named = [
        "schema.max_blocks",
        "schema.fields[0].pattern",
        "schema.blocks[0].max_count",
        "schema.tables.events.row_count_min",
        "schema.tables.events.columns[0].monotonic",
        "schema.arrays.lyso_map.range",
    ];
    for (path, named) in cases.into_iter().zip(named) {
        let mut s = base_schema();
        set(&mut s, path, json!(1));
        match validate_manifest(&with_schema(s)) {
            Err(Error::NewerFormatFields { fields }) => assert_eq!(fields, [named]),
            other => panic!("{path:?}: an unknown schema rule must be refused, got {other:?}"),
        }
    }
}
