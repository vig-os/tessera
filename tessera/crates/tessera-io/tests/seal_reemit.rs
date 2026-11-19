//! A product whose manifest the typed view can't reproduce (a newer-format field, `"study": null`)
//! survives every tessera path that re-emits a product under its seal, and is refused by every path
//! that would re-emit the view instead (#573, SPEC §4).

use serde_json::Value;
use tessera_core::{Error, Manifest};
use tessera_io::stored::canonical_form;
use tessera_io::{pack, pack_dir, unpack, Reader, Repository};

/// `empty_no_blocks` from the corpus, with `edit` applied and resealed per SPEC §4.
fn resealed(edit: impl FnOnce(&mut Value)) -> String {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../corpus/files/empty_no_blocks.tsra"
    );
    let mut v: Value = serde_json::from_str(Reader::open(path.as_ref()).unwrap().manifest_json())
        .expect("corpus manifest parses");
    edit(&mut v);
    v["manifest_hash"] = Value::String(tessera_core::manifest::seal_of(&v).unwrap());
    serde_json::to_string_pretty(&v).unwrap()
}

fn round_trip(manifest_json: &str) {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("exploded");
    std::fs::create_dir_all(dir.join("blocks")).unwrap();
    std::fs::write(dir.join("manifest.json"), manifest_json).unwrap();

    // Both doors store the JCS form of the object they read (ADR-0066 rule 1, #912): every key it
    // arrived with is kept — the newer field, the explicit `null` — so it verifies under its seal.
    let canonical = String::from_utf8(canonical_form(manifest_json).unwrap()).unwrap();
    let parsed: Value = serde_json::from_str(manifest_json).unwrap();
    assert_eq!(serde_json::from_str::<Value>(&canonical).unwrap(), parsed);

    let packed = tmp.path().join("p.tsra");
    pack_dir(&dir, &packed).unwrap();
    assert_eq!(Reader::open(&packed).unwrap().manifest_json(), canonical);

    let out = tmp.path().join("unpacked");
    unpack(&packed, &out).unwrap();
    let written = std::fs::read_to_string(out.join("manifest.json")).unwrap();
    assert_eq!(
        written, canonical,
        "unpack must write the stored object, in JCS form"
    );
}

#[test]
fn newer_field_and_null_products_round_trip_through_pack_dir_and_unpack() {
    round_trip(&resealed(|v| {
        v["actors"] = serde_json::json!([{"role": "r"}])
    }));
    round_trip(&resealed(|v| v["study"] = Value::Null));
}

#[test]
fn the_view_of_a_newer_product_is_not_packed_or_stored() {
    let m =
        Manifest::from_json_verified(&resealed(|v| v["actors"] = serde_json::json!([]))).unwrap();
    assert_eq!(m.newer_fields(), ["actors"]);
    let tmp = tempfile::tempdir().unwrap();
    let err = pack(&m, &[], &tmp.path().join("x.tsra")).unwrap_err();
    assert!(matches!(err, Error::NewerFormatFields { .. }), "{err}");
    let repo = Repository::init(&tmp.path().join("repo")).unwrap();
    let err = repo.put_manifest(&m).unwrap_err();
    assert!(matches!(err, Error::NewerFormatFields { .. }), "{err}");
}

/// A manifest edited after sealing is not packed under its old seal: a writer can't emit a product
/// that fails its own verification.
#[test]
fn a_manifest_edited_after_sealing_is_not_packed() {
    let mut m = Manifest::from_json_verified(&resealed(|_| {})).unwrap();
    m.description = "edited".into();
    let tmp = tempfile::tempdir().unwrap();
    let err = pack(&m, &[], &tmp.path().join("x.tsra")).unwrap_err();
    assert!(matches!(err, Error::Invalid(_)), "{err}");
}
