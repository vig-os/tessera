//! Forward compatibility of collections and embedded product schemas (#873, ADR-0060 §2.2 / §11 P0b).
//!
//! The #573 rule, applied beyond the product manifest: a collection's seal is verified over its
//! stored bytes (SPEC §4, ADR-0066), so a key a newer writer added and sealed verifies and is named, a
//! key spliced in under an old seal is tampering, and a repeated key is refused.
//!
//! The tests edit documents as JSON objects that carry their seal in band under `manifest_hash`, a
//! convention of this file only: [`split`] turns such an object into the stored form (its JCS bytes
//! without that key, and the seal), which is what every reader reads. An embedded schema that
//! carries a rule this build doesn't model is refused at validate rather than reported valid.

use serde_json::{json, Value};
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
    with_seal(b.seal().unwrap().to_stored().unwrap())
}

/// A stored document as this file's test object: its bytes parsed, with the seal in band.
fn with_seal((bytes, seal): (Vec<u8>, String)) -> Value {
    let mut v: Value = serde_json::from_slice(&bytes).unwrap();
    v["manifest_hash"] = json!(seal);
    v
}

/// The stored form of a test object: its JCS bytes without `manifest_hash`, and the seal it carries
/// (empty when it carries none, which no stored bytes hash to).
fn split(v: &Value) -> (Vec<u8>, String) {
    let mut bare = v.clone();
    let seal = bare
        .as_object_mut()
        .and_then(|o| o.remove("manifest_hash"))
        .and_then(|s| s.as_str().map(str::to_owned))
        .unwrap_or_default();
    (serde_jcs::to_vec(&bare).unwrap(), seal)
}

/// Seal `v` the way SPEC §4 defines it: `blake3` over its stored bytes.
fn reseal(v: &mut Value) {
    v["manifest_hash"] = json!(tessera_core::hash::digest(&split(v).0));
}

fn verify(v: &Value) -> tessera_core::Result<Collection> {
    let (bytes, seal) = split(v);
    Collection::from_stored(&bytes, &seal)
}

fn verify_manifest(v: &Value) -> tessera_core::Result<Manifest> {
    let (bytes, seal) = split(v);
    Manifest::from_stored(&bytes, &seal)
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
    match last.parse::<usize>() {
        Ok(i) => at[i] = value,
        Err(_) => at[*last] = value,
    }
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

    // A deleted seal doesn't launder a splice: without its `collection.seal` the collection is not
    // read at all (the alpha layout, ADR-0066 rule 10) ...
    let mut spliced = sealed.clone();
    spliced["forged"] = json!(true);
    let (bytes, _) = split(&spliced);
    assert!(matches!(
        Collection::from_stored_members(&bytes, None),
        Err(Error::UnsupportedLayout { .. })
    ));
    // ... and a splice that deletes `content_hash` and reseals is refused for the missing hash.
    spliced.as_object_mut().unwrap().remove("content_hash");
    reseal(&mut spliced);
    assert_eq!(integrity(verify(&spliced)), "content_hash");
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
        match Collection::from_stored(dup.as_bytes(), "") {
            Err(Error::Serde(e)) => assert!(e.to_string().contains("duplicate key"), "{e}"),
            other => panic!("a duplicate key must be refused, got {other:?}"),
        }
    }
}

/// A product embedding `schema`, as a writer whose build models every rule in it would have sealed
/// it. This build refuses to seal a schema it cannot read
/// (`sealing_refuses_a_schema_this_build_cannot_read`), so the schema is attached to a sealed
/// product instead: `validate_manifest` reads the schema, never the seal.
fn with_schema(schema: Value) -> Manifest {
    let mut m = ProductBuilder::new("custom", "p", "d", TS).seal().unwrap();
    m.schema = Some(schema);
    m
}

/// Sealing checks the embedded schema's rules (#895), so a schema this build can't read is refused
/// at seal as at validate (ADR-0060 §2.2): a rule from a newer revision, a record in array form.
/// Sealing against either would ignore a rule the schema declares.
#[test]
fn sealing_refuses_a_schema_this_build_cannot_read() {
    let seal = |schema: Value| {
        let mut b = ProductBuilder::new("custom", "p", "d", TS);
        b.with_schema(schema);
        b.seal()
    };
    seal(base_schema()).expect("a schema this build models seals");
    let mut newer = base_schema();
    newer["fields"][0]["pattern"] = json!("^[A-Z]+$");
    match seal(newer) {
        Err(Error::NewerFormatFields { fields }) => {
            assert_eq!(fields, ["schema.fields[0].pattern"]);
        }
        other => panic!("a newer rule must not seal, got {other:?}"),
    }
    let mut array_form = base_schema();
    array_form["fields"] = json!([["site", "acquisition site", "string"]]);
    assert!(matches!(seal(array_form), Err(Error::Invalid(_))));
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

/// `Err(Serde)` or `Err(Invalid)` naming `path` as `got` (`"an array"`) where the format defines
/// `declared` (`"an object"`).
fn assert_form_refused<T: std::fmt::Debug>(
    r: tessera_core::Result<T>,
    path: &str,
    got: &str,
    declared: &str,
) {
    let msg = match r {
        Err(Error::Serde(e)) => e.to_string(),
        Err(Error::Invalid(m)) => m,
        other => panic!("{path}: the undeclared spelling must be refused, got {other:?}"),
    };
    let at = if path.is_empty() {
        "the top-level value".to_owned()
    } else {
        format!("`{path}`")
    };
    let want = format!("{at} is {got} where the format defines {declared}");
    assert!(msg.contains(&want), "{path}: {msg}");
}

fn assert_array_form_refused<T: std::fmt::Debug>(r: tessera_core::Result<T>, path: &str) {
    assert_form_refused(r, path, "an array", "an object");
}

/// The sealed formats define every struct as a JSON object (SPEC §4/§5). serde would also read a
/// struct from an array positionally, so an array-form parent hid its unknown keys from the
/// newer-field walk and gave one value a second spelling: every such array is refused.
#[test]
fn embedded_schema_array_form_is_refused() {
    use tessera_core::ProductSchema;
    let identity_with = |extra: (&str, Value)| {
        let mut t = json!({ "kind": "identity" });
        t[extra.0] = extra.1;
        t
    };
    // (where, how the array hides a newer rule, the array's path).
    let cases: Vec<(&[&str], Value, &str)> = vec![
        // A table contract as an array: its column carries `min`, a rule this build doesn't model.
        (
            &["tables", "events"],
            json!([[{ "name": "t", "dtype": "u8", "min": 0 }]]),
            "tables.events",
        ),
        // A reference as an array hiding `to.strict`.
        (
            &["tables", "events", "references"],
            json!([[["t"], { "scope": "block", "block": "b", "columns": ["t"], "strict": true }]]),
            "tables.events.references[0]",
        ),
        // A column as an array hiding `referencing.weird`.
        (
            &["tables", "events", "columns", "0"],
            json!(["t", "u8", null, null, null, null, null, false,
                   { "transform": { "kind": "identity" }, "weird": 1 }]),
            "tables.events.columns[0]",
        ),
        // A column as an array hiding `referencing.transform.x`.
        (
            &["tables", "events", "columns", "0"],
            json!(["t", "u8", null, null, null, null, null, false,
                   { "transform": identity_with(("x", json!(1))) }]),
            "tables.events.columns[0]",
        ),
        // The internally tagged transform itself in array form (tag first).
        (
            &["tables", "events", "columns", "0", "referencing"],
            json!({ "transform": ["affine_1d", 1.0, 0.0] }),
            "tables.events.columns[0].referencing.transform",
        ),
        // A field spec, a block requirement and an array contract.
        (
            &["fields", "0"],
            json!(["site", "acquisition site", "string"]),
            "fields[0]",
        ),
        (
            &["blocks", "0"],
            json!(["volume", null, 0, "the volume"]),
            "blocks[0]",
        ),
        (
            &["arrays", "lyso_map"],
            json!(["float32", 2]),
            "arrays.lyso_map",
        ),
    ];
    for (at, value, path) in cases {
        let mut s = base_schema();
        set(&mut s, at, value);
        assert_array_form_refused(ProductSchema::from_value(&s), path);
        assert_array_form_refused(ProductSchema::view_of(&s), path);
        // validate_manifest reports it as an embedded schema that doesn't parse.
        assert_array_form_refused(validate_manifest(&with_schema(s)), path);
    }

    // The whole schema as an array, hiding `fields[0].pattern`.
    let whole = json!(["custom", "1", "d",
        [{ "id": "site", "description": "s", "dtype": "string", "pattern": "^A" }]]);
    assert_array_form_refused(ProductSchema::from_value(&whole), "");
    assert_array_form_refused(validate_manifest(&with_schema(whole)), "");

    // The object form of the same rules still parses (and names the newer rule).
    let mut object = base_schema();
    object["tables"]["events"]["columns"][0]["referencing"] =
        json!({ "transform": { "kind": "affine_1d", "slope": 1.0, "intercept": 0.0 } });
    validate_manifest(&with_schema(object)).expect("the object form validates");
}

#[test]
fn collection_array_form_is_refused() {
    let sealed = sealed_collection();
    // `level` and `members[0]` written positionally: the same collection, spelled a second way.
    let level = sealed["level"].clone();
    let m = &sealed["members"][0];
    let cases = [
        (
            &["level"][..],
            json!([level["_vocabulary"], level["_code"]]),
            "level",
        ),
        (
            &["members", "0"],
            json!([m["reference"], m["manifest_hash"], m["role"]]),
            "members[0]",
        ),
    ];
    for (at, value, path) in cases {
        let mut arr = sealed.clone();
        set(&mut arr, at, value);
        reseal(&mut arr);
        assert_array_form_refused(verify(&arr), path);
        assert_array_form_refused(Collection::from_json(&arr.to_string()), path);
    }
}

#[test]
fn manifest_array_form_is_refused() {
    let p = ProductBuilder::new("recon", "p", "d", TS).seal().unwrap();
    let sealed = with_seal(p.to_stored().unwrap());
    verify_manifest(&sealed).expect("the writer's own manifest verifies");
    let producer = sealed["producer"].clone();
    let cases = [
        (
            "producer",
            json!([producer["tool"], producer["version"]]),
            "producer",
        ),
        ("generation", json!([{ "seed": 1 }]), "generation"),
        ("sources", json!([["raw", "blake3:00"]]), "sources[0]"),
        (
            "ingest_transform",
            json!([["dictionary_materialised", {}]]),
            "ingest_transform[0]",
        ),
    ];
    for (key, value, path) in cases {
        let mut arr = sealed.clone();
        arr[key] = value;
        reseal(&mut arr);
        assert_array_form_refused(verify_manifest(&arr), path);
    }
    // The whole manifest as an array of its values in field order.
    let order = [
        "tessera_version",
        "id",
        "id_inputs",
        "product",
        "name",
        "description",
        "timestamp",
        "producer",
        "generation",
        "schema",
        "blocks",
        "sources",
        "study",
        "metadata",
        "extra",
        "ingest_transform",
        "content_hash",
    ];
    // Absent keys take the value their field defaults to, so the array is otherwise well-typed.
    let default = |k: &str| match k {
        "id_inputs" | "metadata" | "extra" => json!({}),
        "blocks" | "sources" | "ingest_transform" => json!([]),
        _ => Value::Null,
    };
    let whole = Value::Array(
        order
            .iter()
            .map(|k| sealed.get(*k).cloned().unwrap_or_else(|| default(k)))
            .collect(),
    );
    assert_array_form_refused(Manifest::from_json(&whole.to_string()), "");
}

/// serde also reads a unit enum variant from a one-key object (`{"raw": null}` for `"raw"`): a second
/// spelling of one sealed value, refused like the array form.
#[test]
fn unit_variant_object_form_is_refused() {
    use tessera_core::ProductSchema;
    let sealed = sealed_collection();
    for (at, value, path) in [
        (
            &["members", "0", "role"][..],
            json!({ "raw": null }),
            "members[0].role",
        ),
        (
            &["members", "1", "kind"],
            json!({ "collection": null }),
            "members[1].kind",
        ),
    ] {
        let mut c = sealed.clone();
        set(&mut c, at, value);
        reseal(&mut c);
        assert_form_refused(verify(&c), path, "an object", "a string");
    }

    for (at, value, path) in [
        (
            &["blocks", "0", "kind"][..],
            json!({ "array": null }),
            "blocks[0].kind",
        ),
        (
            &["tables", "events", "order"],
            json!({ "producer": null }),
            "tables.events.order",
        ),
        (
            &["fields", "0", "sensitivity"],
            json!({ "identifying": null }),
            "fields[0].sensitivity",
        ),
    ] {
        let mut s = base_schema();
        set(&mut s, at, value);
        assert_form_refused(ProductSchema::from_value(&s), path, "an object", "a string");
    }
}

/// A key the serializer skips when it holds its default (`skip_serializing_if`) is still compared
/// with what the view holds: before, the spelling check walked only the keys the view wrote back,
/// so a default unit variant spelled as an object (`{"product": null}` for the member `kind`
/// `"product"`) verified, under a seal of its own, as a second sealed spelling of one collection.
#[test]
fn a_skipped_default_spelled_as_an_object_is_refused() {
    use tessera_core::ProductSchema;
    let mut c = sealed_collection();
    set(
        &mut c,
        &["members", "0", "kind"],
        json!({ "product": null }),
    );
    reseal(&mut c);
    assert_form_refused(verify(&c), "members[0].kind", "an object", "a default");

    let mut s = base_schema();
    for (at, value, path) in [
        (
            &["tables", "events", "order"][..],
            json!({ "sequence_key": null }),
            "tables.events.order",
        ),
        (
            &["tables", "events", "columns", "0", "sensitivity"],
            json!({ "public": null }),
            "tables.events.columns[0].sensitivity",
        ),
    ] {
        let mut s = s.clone();
        set(&mut s, at, value);
        assert_form_refused(
            ProductSchema::from_value(&s),
            path,
            "an object",
            "a default",
        );
    }
    set(
        &mut s,
        &["tables", "events", "order"],
        json!("sequence_key"),
    );
    ProductSchema::from_value(&s).expect("the default unit variant as its string");
}

/// One case per kind of `skip_serializing_if` the manifest, collection and schema types use. The
/// explicit canonical default is a present key (SPEC §4: hashed as received, never a newer field),
/// so it is accepted under a seal of its own; any other spelling of that default is refused.
#[test]
fn every_kind_of_skipped_default_accepts_only_its_own_spelling() {
    use tessera_core::ProductSchema;
    // (kind of skip, path, the explicit default, another spelling)
    let collection: [(&str, &[&str], Value, Value); 3] = [
        (
            "unit variant",
            &["members", "0", "kind"],
            json!("product"),
            json!({ "product": null }),
        ),
        ("Option::None", &["study"], Value::Null, json!({})),
        (
            "empty Vec",
            &["members", "0", "derived_from"],
            json!([]),
            json!({}),
        ),
    ];
    let sealed = sealed_collection();
    for (kind, at, explicit, other) in collection {
        let mut c = sealed.clone();
        set(&mut c, at, explicit);
        reseal(&mut c);
        let got = verify(&c).unwrap_or_else(|e| panic!("{kind}: explicit default refused: {e}"));
        assert!(
            got.newer_fields().is_empty(),
            "{kind}: {:?}",
            got.newer_fields()
        );
        assert_ne!(
            c["manifest_hash"], sealed["manifest_hash"],
            "{kind}: a seal of its own"
        );
        assert_eq!(
            c["content_hash"], sealed["content_hash"],
            "{kind}: the same members"
        );
        set(&mut c, at, other);
        reseal(&mut c);
        assert!(verify(&c).is_err(), "{kind}: another spelling verified");
    }

    let manifest: [(&str, &str, Value, Value); 2] = [
        ("empty map", "metadata", json!({}), json!([])),
        ("empty Vec", "ingest_transform", json!([]), json!({})),
    ];
    let p = ProductBuilder::new("recon", "p", "d", TS).seal().unwrap();
    let sealed = with_seal(p.to_stored().unwrap());
    for (kind, key, explicit, other) in manifest {
        let mut m = sealed.clone();
        m[key] = explicit;
        reseal(&mut m);
        let got =
            verify_manifest(&m).unwrap_or_else(|e| panic!("{kind}: explicit default refused: {e}"));
        assert!(got.newer_fields().is_empty(), "{kind}");
        m[key] = other;
        reseal(&mut m);
        assert!(verify_manifest(&m).is_err(), "{kind}: other spelling");
    }

    let schema: [(&str, &[&str], Value, Value); 4] = [
        (
            "unit variant",
            &["tables", "events", "order"],
            json!("sequence_key"),
            json!({ "sequence_key": null }),
        ),
        (
            "Option::None",
            &["fields", "0", "unit"],
            Value::Null,
            json!({}),
        ),
        (
            "empty Vec",
            &["tables", "events", "keys"],
            json!([]),
            json!({}),
        ),
        ("false", &["fields", "0", "inherit"], json!(false), json!(0)),
    ];
    for (kind, at, explicit, other) in schema {
        let mut s = base_schema();
        set(&mut s, at, explicit);
        ProductSchema::from_value(&s).unwrap_or_else(|e| panic!("{kind}: explicit default: {e}"));
        set(&mut s, at, other);
        assert!(
            ProductSchema::from_value(&s).is_err(),
            "{kind}: another spelling validated"
        );
    }
    let mut s = base_schema();
    s["arrays"] = json!({});
    ProductSchema::from_value(&s).expect("empty map: the explicit default");
    s["arrays"] = json!([]);
    assert!(
        ProductSchema::from_value(&s).is_err(),
        "empty map: another spelling validated"
    );
}

/// A newer field is decided by structure, never by a string spelling of its path (#873): a key is
/// exempt from the spelling check only where its parent type doesn't declare it. Before, the exemption
/// matched dotted path strings, which are not injective: a top-level key literally named
/// `members[0].kind` (a newer field) spelled the same path as the member's declared `kind`, so that
/// `kind` in the object form `{"product": null}` escaped the check and verified under a seal of its
/// own.
#[test]
fn a_key_spelling_a_declared_path_cannot_exempt_it() {
    let mut c = sealed_collection();
    c["members[0].kind"] = json!(1);
    reseal(&mut c);
    let got = verify(&c).expect("a crafted key alone is a sealed newer field");
    assert_eq!(got.newer_fields(), [r#"["members[0].kind"]"#]);

    set(
        &mut c,
        &["members", "0", "kind"],
        json!({ "product": null }),
    );
    reseal(&mut c);
    assert_form_refused(verify(&c), "members[0].kind", "an object", "a default");

    use tessera_core::ProductSchema;
    let mut s = base_schema();
    s["tables.events.order"] = json!(1);
    let (_, newer) = ProductSchema::view_of(&s).expect("a crafted key alone is a newer rule");
    assert_eq!(newer, [r#"["tables.events.order"]"#]);
    set(
        &mut s,
        &["tables", "events", "order"],
        json!({ "sequence_key": null }),
    );
    assert_form_refused(
        ProductSchema::view_of(&s),
        "tables.events.order",
        "an object",
        "a default",
    );
}

/// Newer-field paths are reported unambiguously: a key holding `.`, `[` or `"` is quoted, so a
/// crafted top-level key never reads as the nested declared or newer field it spells (#873).
#[test]
fn newer_field_paths_are_reported_unambiguously() {
    let p = ProductBuilder::new("recon", "p", "d", TS).seal().unwrap();
    let mut m = with_seal(p.to_stored().unwrap());
    m["producer"]["agents"] = json!(["x"]);
    m["producer.agents"] = json!(1);
    m["producer.tool"] = json!(1);
    reseal(&mut m);
    let got = verify_manifest(&m).expect("sealed newer fields verify");
    let mut newer = got.newer_fields().to_vec();
    newer.sort();
    assert_eq!(
        newer,
        [
            r#"["producer.agents"]"#,
            r#"["producer.tool"]"#,
            "producer.agents"
        ]
    );

    // The schema's own paths are prefixed by its key in the manifest structurally, so a crafted key
    // stays quoted under `schema` too.
    let mut s = base_schema();
    s["tables.events.order"] = json!(1);
    s["tables"]["events"]["row_count_min"] = json!(1);
    match validate_manifest(&with_schema(s)) {
        Err(Error::NewerFormatFields { fields }) => assert_eq!(
            fields,
            [
                r#"schema["tables.events.order"]"#,
                "schema.tables.events.row_count_min"
            ]
        ),
        other => panic!("a newer rule must not validate, got {other:?}"),
    }
}
